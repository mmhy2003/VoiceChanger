//! Seeded random mutations of small `.pth` fixtures, run on every
//! `cargo test`: the readers must return `Ok` or `Err`, never panic or hang.
//! The ignored `write_fuzz_seeds` writes the unmutated fixtures as the
//! cargo-fuzz seed corpus.

use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::ConvertError;
use crate::checkpoint::Checkpoint;
use crate::pickle::{self, DType};
use crate::pth::PthArchive;
use crate::testutil::{Fixture, P, f32_bytes, pickle, rvc_pth, torch_zip, zip_of};

/// Longest time one mutated input may take; longer counts as a hang.
const BUDGET: Duration = Duration::from_secs(1);
/// Bytes of a failing input shown in hex.
const HEX_BYTES: usize = 256;
/// Bytes the overwrite mutator picks from: edge values and the BINGET,
/// MARK and STOP opcodes.
const INTERESTING: [u8; 7] = [0x00, 0xff, 0x7f, 0x80, b'h', b'(', b'.'];

/// An unmutated fixture: the zip, the `data.pkl` inside it (inside the
/// inner `.pth` for nested zips) and what each reader gives for it.
struct Case {
    name: &'static str,
    zip: Vec<u8>,
    pkl: Vec<u8>,
    /// Outcome of `pickle::load` on `pkl`, as [`outcome`] names it.
    pickle: &'static str,
    /// Outcome of [`read_archive`] on `zip`.
    archive: &'static str,
}

/// [`Fixture::v2_40k`] with tiny weights (the same names, dtypes and the
/// dims the classification reads), so a fixture is a few KB.
fn small_v2_40k() -> Fixture {
    let mut f = Fixture::v2_40k();
    f.weights = vec![
        ("enc_p.emb_phone.weight".into(), vec![1, 768], DType::F16),
        ("emb_g.weight".into(), vec![3, 4], DType::F16),
        ("dec.conv_pre.weight".into(), vec![2, 3, 7], DType::F16),
        ("enc_q.enc.in_layers.0.bias".into(), vec![5], DType::F16),
    ];
    f
}

/// `data.pkl` of the torch archive `zip`.
fn data_pkl(zip: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip)).expect("open fixture zip");
    let mut pkl = Vec::new();
    zip.by_name("archive/data.pkl")
        .expect("data.pkl")
        .read_to_end(&mut pkl)
        .expect("read data.pkl");
    pkl
}

/// A case whose zip is a torch archive.
fn torch(name: &'static str, zip: Vec<u8>, pickle: &'static str, archive: &'static str) -> Case {
    Case {
        name,
        pkl: data_pkl(&zip),
        zip,
        pickle,
        archive,
    }
}

/// A torch archive whose root is `{"w": t}` over six f32 in storage `0`.
fn one_tensor(shape: Vec<usize>, stride: Vec<usize>, offset: usize) -> Vec<u8> {
    let t = P::Tensor {
        key: "0".into(),
        dtype: DType::F32,
        numel: 6,
        offset,
        shape,
        stride,
    };
    let pkl = pickle(&P::Dict(vec![(P::Str("w".into()), t)]));
    torch_zip(
        "archive",
        &pkl,
        &[("0", f32_bytes(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]))],
    )
}

fn cases() -> Vec<Case> {
    let v2 = small_v2_40k();
    let mut fp32 = small_v2_40k();
    fp32.weights.iter_mut().for_each(|w| w.2 = DType::F32);
    let mut v1 = small_v2_40k();
    v1.version = Some("v1");
    let mut training = small_v2_40k();
    training.extra.push(("optimizer", P::Dict(Vec::new())));
    let mut no_f0 = small_v2_40k();
    no_f0.f0 = Some(0);

    // GLOBAL os system, a tuple, REDUCE; a list holds the call.
    let os_system = pickle(&P::List(vec![
        P::Global("os".into(), "system".into()),
        P::Tuple(vec![P::Str("calc".into())]),
        P::Raw(b"R".to_vec()),
    ]));

    let pth = rvc_pth(&v2);
    vec![
        torch("v2_40k_fp16", rvc_pth(&v2), "Ok", "Ok"),
        torch("v2_40k_fp32", rvc_pth(&fp32), "Ok", "Ok"),
        torch("v1", rvc_pth(&v1), "Ok", "RvcV1"),
        torch(
            "training_keys",
            rvc_pth(&training),
            "Ok",
            "TrainingCheckpoint",
        ),
        torch("no_f0", rvc_pth(&no_f0), "Ok", "NoF0"),
        torch(
            "os_system",
            torch_zip("archive", &os_system, &[]),
            "UnsafeContent",
            "UnsafeContent",
        ),
        torch(
            "non_contiguous",
            one_tensor(vec![2, 3], vec![1, 2], 0),
            "UnsafeContent",
            "UnsafeContent",
        ),
        torch(
            "offset_out_of_range",
            one_tensor(vec![2, 3], vec![3, 1], 1),
            "UnsafeContent",
            "UnsafeContent",
        ),
        Case {
            name: "nested_one",
            zip: zip_of(&[("voice/voice.pth", &pth), ("voice/added.index", b"idx")]),
            pkl: data_pkl(&pth),
            pickle: "Ok",
            archive: "Ok",
        },
        Case {
            name: "nested_two",
            zip: zip_of(&[("a.pth", &pth), ("b.pth", &pth)]),
            pkl: data_pkl(&pth),
            pickle: "Ok",
            archive: "SeveralModels",
        },
    ]
}

/// What the `.pth` fuzz target does: open, classify, then read every weight.
fn read_archive(bytes: &[u8]) -> Result<(), ConvertError> {
    let mut archive = PthArchive::open(bytes.to_vec(), "voice.pth")?;
    let ckpt = Checkpoint::from_value(&archive.root, &archive.name)?;
    for t in ckpt.weights.values() {
        archive.tensor_f32(t)?;
    }
    Ok(())
}

/// The variant name of `r`.
fn outcome<T>(r: &Result<T, ConvertError>) -> &'static str {
    match r {
        Ok(_) => "Ok",
        Err(ConvertError::NotPyTorch) => "NotPyTorch",
        Err(ConvertError::UnsafeContent(_)) => "UnsafeContent",
        Err(ConvertError::RvcV1) => "RvcV1",
        Err(ConvertError::TrainingCheckpoint) => "TrainingCheckpoint",
        Err(ConvertError::NoF0) => "NoF0",
        Err(ConvertError::SeveralModels) => "SeveralModels",
        Err(ConvertError::NoTemplate(_)) => "NoTemplate",
        Err(ConvertError::ValidationFailed(_)) => "ValidationFailed",
        Err(ConvertError::DiskFull) => "DiskFull",
        Err(ConvertError::Cancelled) => "Cancelled",
        Err(ConvertError::Io(_)) => "Io",
    }
}

/// `pool[own]` changed by one random mutator.
fn mutate(rng: &mut StdRng, pool: &[Vec<u8>], own: usize) -> Vec<u8> {
    let mut out = pool[own].clone();
    match rng.random_range(0..4) {
        // Flip 1–8 random bits.
        0 if !out.is_empty() => {
            for _ in 0..rng.random_range(1..=8) {
                let at = rng.random_range(0..out.len());
                out[at] ^= 1 << rng.random_range(0..8);
            }
        }
        // Truncate at a random length.
        1 => out.truncate(rng.random_range(0..=out.len())),
        // Splice a random slice of another fixture at a random position.
        2 if pool.len() > 1 => {
            let mut other = rng.random_range(0..pool.len() - 1);
            if other >= own {
                other += 1;
            }
            let other = &pool[other];
            let start = rng.random_range(0..=other.len());
            let end = rng.random_range(start..=other.len());
            let at = rng.random_range(0..=out.len());
            out.splice(at..at, other[start..end].iter().copied());
        }
        // Overwrite 1–4 bytes with interesting values.
        3 if !out.is_empty() => {
            for _ in 0..rng.random_range(1..=4) {
                let at = rng.random_range(0..out.len());
                out[at] = INTERESTING[rng.random_range(0..INTERESTING.len())];
            }
        }
        _ => {}
    }
    out
}

/// The first [`HEX_BYTES`] of `bytes` in hex.
fn hex(bytes: &[u8]) -> String {
    let shown: String = bytes
        .iter()
        .take(HEX_BYTES)
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() > HEX_BYTES {
        format!("{shown}… ({} bytes)", bytes.len())
    } else {
        shown
    }
}

/// Runs `read` on `iterations` mutated inputs, iteration `i` from
/// `StdRng::seed_from_u64(seed + i)` over fixture `i % pool.len()`. Fails on
/// a panic, or on an input that takes [`BUDGET`] or longer.
fn hammer(what: &str, seed: u64, iterations: usize, pool: &[Vec<u8>], read: impl Fn(&[u8])) {
    for i in 0..iterations {
        let case_seed = seed.wrapping_add(u64::try_from(i).expect("iteration fits in u64"));
        let mut rng = StdRng::seed_from_u64(case_seed);
        let input = mutate(&mut rng, pool, i % pool.len());
        let start = Instant::now();
        let result = panic::catch_unwind(AssertUnwindSafe(|| read(&input)));
        let took = start.elapsed();
        assert!(
            result.is_ok(),
            "{what} panicked: seed {case_seed}, iteration {i}, input {}",
            hex(&input)
        );
        assert!(
            took < BUDGET,
            "{what} took {took:?}: seed {case_seed}, iteration {i}, input {}",
            hex(&input)
        );
    }
}

fn pickles() -> Vec<Vec<u8>> {
    cases().into_iter().map(|c| c.pkl).collect()
}

fn archives() -> Vec<Vec<u8>> {
    cases().into_iter().map(|c| c.zip).collect()
}

fn load_pickle(bytes: &[u8]) {
    let _ = pickle::load(bytes);
}

fn load_archive(bytes: &[u8]) {
    let _ = read_archive(bytes);
}

#[test]
fn unmutated_fixtures_behave() {
    for c in cases() {
        assert_eq!(
            outcome(&pickle::load(&c.pkl)),
            c.pickle,
            "{} pickle",
            c.name
        );
        assert_eq!(
            outcome(&read_archive(&c.zip)),
            c.archive,
            "{} archive",
            c.name
        );
    }
}

#[test]
fn mutated_pickles_never_panic() {
    hammer("pickle::load", 0x5eed_0000, 4_000, &pickles(), load_pickle);
}

#[test]
fn mutated_archives_never_panic() {
    hammer(
        "read_archive",
        0x5eed_1000_0000,
        1_000,
        &archives(),
        load_archive,
    );
}

/// Different seeds from the tests above, with the same per-input budget,
/// so a hang that the never-panic seeds miss can still show up.
#[test]
fn mutated_inputs_finish_quickly() {
    hammer("pickle::load", 0xfa57_0000, 1_000, &pickles(), load_pickle);
    hammer(
        "read_archive",
        0xfa57_1000_0000,
        250,
        &archives(),
        load_archive,
    );
}

/// Writes every unmutated fixture as a cargo-fuzz seed:
/// `fuzz/corpus/pickle/<name>` (its data.pkl) and `fuzz/corpus/pth/<name>`.
#[test]
#[ignore = "writes the committed seed corpus; run by hand"]
fn write_fuzz_seeds() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("corpus");
    for (dir, pick) in [
        ("pickle", (|c: &Case| c.pkl.clone()) as fn(&Case) -> Vec<u8>),
        ("pth", |c: &Case| c.zip.clone()),
    ] {
        let dir = corpus.join(dir);
        std::fs::create_dir_all(&dir).expect("create corpus folder");
        for c in cases() {
            std::fs::write(dir.join(c.name), pick(&c)).expect("write seed");
        }
    }
}
