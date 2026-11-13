//! ONNX Runtime loading, execution provider selection and automatic CPU fallback.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use ort::environment::Environment;
use ort::ep;
use ort::session::Session;
use ort::session::builder::SessionBuilder;

use crate::assets::{self, DML_DLL, ORT_DLL};
use crate::error::{EngineError, Result};

/// Execution provider a session is asked to run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    DirectMl { device: i32 },
    Cuda { device: i32 },
    Cpu,
}

impl Default for Provider {
    fn default() -> Self {
        Provider::DirectMl { device: 0 }
    }
}

/// Provider a session actually runs on.
pub type ActiveProvider = Provider;

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::DirectMl { device } => write!(f, "DirectML (GPU {device})"),
            Provider::Cuda { device } => write!(f, "CUDA (GPU {device})"),
            Provider::Cpu => f.write_str("CPU"),
        }
    }
}

/// Tries `provider`, then the CPU. Returns the first success and where it runs,
/// or the last failure reason.
pub fn with_fallback<S>(
    provider: Provider,
    mut build: impl FnMut(Provider) -> std::result::Result<S, String>,
) -> std::result::Result<(S, ActiveProvider), String> {
    let attempts: &[Provider] = if provider == Provider::Cpu {
        &[Provider::Cpu]
    } else {
        &[provider, Provider::Cpu]
    };
    let mut last = String::new();
    for &p in attempts {
        match build(p) {
            Ok(s) => return Ok((s, p)),
            Err(reason) => {
                if p != Provider::Cpu {
                    tracing::warn!(provider = %p, %reason, "execution provider failed, using CPU");
                }
                last = reason;
            }
        }
    }
    Err(last)
}

/// Process-wide ONNX Runtime environment, loaded once from `vendor/runtime/onnxruntime.dll`.
pub fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    let ort_dll = assets::asset(ORT_DLL)?;
    ENV.get_or_init(|| {
        // Windows does not search onnxruntime.dll's folder for its dependencies,
        // so load the vendored DirectML.dll first.
        if let Ok(dml) = assets::asset(DML_DLL) {
            ort::util::preload_dylib(dml.as_os_str())
                .map_err(|e| format!("could not load {}: {e}", dml.display()))?;
        }
        ort::init_from(&ort_dll)
            .map_err(|e| e.to_string())?
            .with_name("vc-engine")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| EngineError::RuntimeInit(e.clone()))
}

/// Opens the model at `path` on `provider`, falling back to the CPU.
pub fn open_session(path: &Path, provider: Provider) -> Result<(Session, ActiveProvider)> {
    let env = environment()?;
    with_fallback(provider, |p| {
        builder(env, p)?
            .commit_from_file(path)
            .map_err(|e| e.to_string())
    })
    .map_err(|r| EngineError::RuntimeInit(format!("could not open {}: {r}", path.display())))
}

/// Opens an in-memory model on `provider`, falling back to the CPU.
pub fn open_session_from_memory(
    bytes: &[u8],
    provider: Provider,
) -> Result<(Session, ActiveProvider)> {
    let env = environment()?;
    with_fallback(provider, |p| {
        builder(env, p)?
            .commit_from_memory(bytes)
            .map_err(|e| e.to_string())
    })
    .map_err(|r| EngineError::RuntimeInit(format!("could not open in-memory model: {r}")))
}

fn builder(env: &Environment, provider: Provider) -> std::result::Result<SessionBuilder, String> {
    let b = Session::builder(env).map_err(|e| e.to_string())?;
    match provider {
        // DirectML supports neither memory patterns nor parallel execution.
        Provider::DirectMl { device } => b
            .with_memory_pattern(false)
            .map_err(|e| e.to_string())?
            .with_parallel_execution(false)
            .map_err(|e| e.to_string())?
            .with_execution_providers([ep::DirectML::default()
                .with_device_id(device)
                .build()
                .error_on_failure()])
            .map_err(|e| e.to_string()),
        Provider::Cuda { device } => b
            .with_execution_providers([ep::CUDA::default()
                .with_device_id(device)
                .build()
                .error_on_failure()])
            .map_err(|e| e.to_string()),
        Provider::Cpu => Ok(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::RMVPE;
    use crate::testutil::assets_or_skip;

    /// Runs `with_fallback` from the default provider, answering attempt `i` with `outcomes[i]`.
    fn record(
        outcomes: &[std::result::Result<u32, &str>],
    ) -> (
        std::result::Result<(u32, ActiveProvider), String>,
        Vec<Provider>,
    ) {
        let mut tried = Vec::new();
        let got = with_fallback(Provider::default(), |p| {
            let i = tried.len();
            tried.push(p);
            outcomes[i].map_err(str::to_string)
        });
        (got, tried)
    }

    #[test]
    fn runtime_loads_and_opens_rmvpe_on_cpu() {
        if !assets_or_skip(&[ORT_DLL, RMVPE]) {
            return;
        }
        let (session, active) =
            open_session(&assets::asset(RMVPE).unwrap(), Provider::Cpu).unwrap();
        assert_eq!(active, Provider::Cpu);
        let shape = session.inputs()[0].dtype().tensor_shape().unwrap();
        assert_eq!(shape.len(), 3, "{shape:?}");
    }

    #[test]
    #[ignore = "needs a DirectML-capable GPU"]
    fn dml_smoke() {
        let rmvpe = assets::asset(RMVPE).unwrap();
        let (_session, active) = open_session(&rmvpe, Provider::DirectMl { device: 0 }).unwrap();
        println!("RMVPE runs on {active}");
    }

    #[test]
    fn cpu_requested_tries_once() {
        let mut tried = Vec::new();
        let got = with_fallback(Provider::Cpu, |p| {
            tried.push(p);
            Err::<(), _>("no".to_string())
        });
        assert_eq!(got, Err("no".to_string()));
        assert_eq!(tried, [Provider::Cpu]);
    }

    #[test]
    fn dml_success_stays_on_dml() {
        let (got, tried) = record(&[Ok(7)]);
        assert_eq!(got, Ok((7, Provider::DirectMl { device: 0 })));
        assert_eq!(tried, [Provider::DirectMl { device: 0 }]);
    }

    #[test]
    fn dml_failure_falls_back_to_cpu() {
        let (got, tried) = record(&[Err("dml broke"), Ok(3)]);
        assert_eq!(got, Ok((3, Provider::Cpu)));
        assert_eq!(tried, [Provider::DirectMl { device: 0 }, Provider::Cpu]);
    }

    #[test]
    fn cuda_failure_falls_back_to_cpu() {
        let mut tried = Vec::new();
        let got = with_fallback(Provider::Cuda { device: 1 }, |p| {
            tried.push(p);
            match p {
                Provider::Cpu => Ok(5),
                _ => Err("no cuda".to_string()),
            }
        });
        assert_eq!(got, Ok((5, Provider::Cpu)));
        assert_eq!(tried, [Provider::Cuda { device: 1 }, Provider::Cpu]);
    }

    #[test]
    fn all_fail_returns_last_reason() {
        let (got, tried) = record(&[Err("dml broke"), Err("cpu broke")]);
        assert_eq!(got, Err("cpu broke".to_string()));
        assert_eq!(tried.len(), 2);
    }

    #[test]
    fn display_names() {
        assert_eq!(
            Provider::DirectMl { device: 0 }.to_string(),
            "DirectML (GPU 0)"
        );
        assert_eq!(Provider::Cuda { device: 0 }.to_string(), "CUDA (GPU 0)");
        assert_eq!(Provider::Cpu.to_string(), "CPU");
    }
}
