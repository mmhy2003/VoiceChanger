//! A restricted unpickler for PyTorch checkpoints.
//!
//! It reads the small part of pickle protocol 2 that `torch.save` writes and
//! builds a [`Value`] tree. Only an allow-list of globals is accepted, and only
//! in the places torch uses them; nothing from the file is ever executed.

use crate::ConvertError;

/// Most opcodes read from one pickle.
pub const MAX_OPS: usize = 10_000_000;
/// Most items on the stack.
pub const MAX_STACK: usize = 1_000_000;
/// Most open MARKs.
pub const MAX_MARKS: usize = 1_024;
/// Most memo entries.
pub const MAX_MEMO: usize = 1_000_000;
/// Longest string or bytes value.
pub const MAX_STRING: usize = 16 << 20;
/// Most items in one list, tuple or dict.
pub const MAX_CONTAINER: usize = 1_000_000;
/// Most nodes copied out of the memo, in total.
pub const MAX_NODES: usize = 10_000_000;
/// Deepest value tree. Deeper trees could overflow the thread's stack when
/// they are cloned, compared or dropped; RVC files are about 4 deep.
pub const MAX_DEPTH: usize = 256;

/// Element type of a tensor's storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DType {
    F32,
    F16,
    BF16,
    I64,
    I32,
    Bool,
}

impl DType {
    pub const ALL: [DType; 6] = [
        DType::F32,
        DType::F16,
        DType::BF16,
        DType::I64,
        DType::I32,
        DType::Bool,
    ];

    /// Bytes per element.
    pub fn size(self) -> usize {
        match self {
            DType::F32 | DType::I32 => 4,
            DType::F16 | DType::BF16 => 2,
            DType::I64 => 8,
            DType::Bool => 1,
        }
    }

    /// Name of the matching class in the `torch` module.
    pub fn storage_name(self) -> &'static str {
        match self {
            DType::F32 => "FloatStorage",
            DType::F16 => "HalfStorage",
            DType::BF16 => "BFloat16Storage",
            DType::I64 => "LongStorage",
            DType::I32 => "IntStorage",
            DType::Bool => "BoolStorage",
        }
    }
}

/// A contiguous tensor inside the storage `data/<key>` of the archive.
#[derive(Clone, Debug, PartialEq)]
pub struct TensorRef {
    pub key: String,
    pub dtype: DType,
    pub storage_numel: usize,
    pub offset: usize,
    pub shape: Vec<usize>,
}

/// An unpickled value. `OrderedDict` becomes `Dict`, keeping order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Tensor(TensorRef),
}

impl Value {
    /// The value for the string key `key` of a dict (the last one, as in Python).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(items) => items
                .iter()
                .rev()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The items of a list or a tuple.
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) | Value::Tuple(items) => Some(items),
            _ => None,
        }
    }
}

/// Reads a pickle. Anything outside the allowed subset, a limit exceeded or
/// truncated input gives [`ConvertError::UnsafeContent`].
pub fn load(bytes: &[u8]) -> Result<Value, ConvertError> {
    Machine::default().run(&mut Cursor { bytes, pos: 0 })
}

/// A global the file may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Allowed {
    OrderedDict,
    RebuildTensorV2,
    RebuildParameter,
    Storage(DType),
}

impl Allowed {
    fn find(module: &[u8], name: &[u8]) -> Option<Self> {
        match (module, name) {
            (b"collections", b"OrderedDict") => Some(Allowed::OrderedDict),
            (b"torch._utils", b"_rebuild_tensor_v2") => Some(Allowed::RebuildTensorV2),
            (b"torch._utils", b"_rebuild_parameter") => Some(Allowed::RebuildParameter),
            (b"torch", name) => DType::ALL
                .into_iter()
                .find(|d| d.storage_name().as_bytes() == name)
                .map(Allowed::Storage),
            _ => None,
        }
    }
}

/// An entry of the stack or the memo.
#[derive(Clone, Debug)]
enum Item {
    /// A value and the depth of its tree (a leaf is 1).
    V {
        value: Value,
        depth: usize,
    },
    Global(Allowed),
    Storage {
        dtype: DType,
        key: String,
        numel: usize,
    },
    /// A tuple holding a `Global` or a `Storage`: a persistent id or the
    /// arguments of `_rebuild_tensor_v2`. It never nests and never goes
    /// into a container.
    Tuple(Vec<Item>),
}

/// String bytes charged as one node when a value is cloned out of the memo.
const BYTES_PER_NODE: usize = 16;

fn bad(msg: impl Into<String>) -> ConvertError {
    ConvertError::UnsafeContent(msg.into())
}

fn limit(what: &str) -> ConvertError {
    bad(format!("limit: {what}"))
}

fn truncated() -> ConvertError {
    bad("truncated")
}

fn leaf(value: Value) -> Item {
    Item::V { value, depth: 1 }
}

/// Raises `depth` to hold a child of depth `child`.
fn nest(depth: &mut usize, child: usize) -> Result<(), ConvertError> {
    let d = child.checked_add(1).ok_or_else(|| limit("depth"))?;
    if d > MAX_DEPTH {
        return Err(limit("depth"));
    }
    *depth = (*depth).max(d);
    Ok(())
}

/// What cloning `value` costs against [`MAX_NODES`].
fn value_cost(value: &Value) -> usize {
    let bytes = |n: usize| 1 + n / BYTES_PER_NODE;
    match value {
        Value::Str(s) => bytes(s.len()),
        Value::Bytes(b) => bytes(b.len()),
        Value::Tensor(t) => bytes(t.key.len()),
        Value::List(items) | Value::Tuple(items) => {
            items.iter().fold(1, |n, v| n.saturating_add(value_cost(v)))
        }
        Value::Dict(items) => items.iter().fold(1, |n, (k, v)| {
            n.saturating_add(value_cost(k))
                .saturating_add(value_cost(v))
        }),
        Value::None | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 1,
    }
}

fn item_cost(item: &Item) -> usize {
    match item {
        Item::V { value, .. } => value_cost(value),
        Item::Global(_) => 1,
        Item::Storage { key, .. } => 1 + key.len() / BYTES_PER_NODE,
        Item::Tuple(items) => items.iter().fold(1, |n, i| n.saturating_add(item_cost(i))),
    }
}

/// Builds a tuple: a plain value, or an [`Item::Tuple`] if it holds globals.
fn tuple(items: Vec<Item>) -> Result<Item, ConvertError> {
    if items.len() > MAX_CONTAINER {
        return Err(limit("container"));
    }
    if items.iter().any(|i| matches!(i, Item::Tuple(_))) {
        return Err(bad("nested tuple of globals"));
    }
    if !items.iter().all(|i| matches!(i, Item::V { .. })) {
        return Ok(Item::Tuple(items));
    }
    let mut depth = 1;
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        if let Item::V { value, depth: d } = item {
            nest(&mut depth, d)?;
            values.push(value);
        }
    }
    Ok(Item::V {
        value: Value::Tuple(values),
        depth,
    })
}

/// Calls an allowed global with `args`.
fn reduce(callable: Item, args: Item) -> Result<Item, ConvertError> {
    match (callable, args) {
        (
            Item::Global(Allowed::OrderedDict),
            Item::V {
                value: Value::Tuple(a),
                ..
            },
        ) if a.is_empty() => Ok(leaf(Value::Dict(Vec::new()))),
        (Item::Global(Allowed::RebuildTensorV2), Item::Tuple(a)) => {
            rebuild_tensor(&a).map(|t| leaf(Value::Tensor(t)))
        }
        (
            Item::Global(Allowed::RebuildParameter),
            Item::V {
                value: Value::Tuple(a),
                ..
            },
        ) => match a.as_slice() {
            [Value::Tensor(t), Value::Bool(_), Value::Dict(_)] => {
                Ok(leaf(Value::Tensor(t.clone())))
            }
            _ => Err(bad("_rebuild_parameter arguments")),
        },
        (Item::Global(g), _) => Err(bad(format!("reduce on {g:?} with these arguments"))),
        _ => Err(bad("reduce on something that isn't a global")),
    }
}

/// `_rebuild_tensor_v2(storage, offset, size, stride, requires_grad, hooks[, metadata])`.
fn rebuild_tensor(args: &[Item]) -> Result<TensorRef, ConvertError> {
    let tensor = |what: &str| bad(format!("tensor {what}"));
    let (dtype, key, storage_numel, offset, size, stride, metadata) = match args {
        [
            Item::Storage { dtype, key, numel },
            Item::V { value: offset, .. },
            Item::V { value: size, .. },
            Item::V { value: stride, .. },
            Item::V {
                value: Value::Bool(_),
                ..
            },
            Item::V {
                value: Value::Dict(_),
                ..
            },
            metadata @ ..,
        ] => (*dtype, key, *numel, offset, size, stride, metadata),
        _ => return Err(tensor("arguments")),
    };
    match metadata {
        []
        | [
            Item::V {
                value: Value::None | Value::Dict(_),
                ..
            },
        ] => {}
        _ => return Err(tensor("metadata")),
    }
    let offset = to_usize(offset).ok_or_else(|| tensor("offset"))?;
    let shape = dims(size).ok_or_else(|| tensor("size"))?;
    let stride = dims(stride).ok_or_else(|| tensor("stride"))?;
    if shape.len() != stride.len() || shape.len() > 8 {
        return Err(tensor("size and stride lengths"));
    }
    // Contiguous strides, as torch computes them: sizes of 0 count as 1.
    let mut expected = 1usize;
    let mut numel = 1usize;
    for (&s, &st) in shape.iter().zip(&stride).rev() {
        if s > 1 && st != expected {
            return Err(tensor("is not contiguous"));
        }
        expected = expected
            .checked_mul(s.max(1))
            .ok_or_else(|| tensor("size overflows"))?;
        numel = numel
            .checked_mul(s)
            .ok_or_else(|| tensor("size overflows"))?;
    }
    let end = offset
        .checked_add(numel)
        .ok_or_else(|| tensor("size overflows"))?;
    if end > storage_numel {
        return Err(tensor("lies outside its storage"));
    }
    Ok(TensorRef {
        key: key.clone(),
        dtype,
        storage_numel,
        offset,
        shape,
    })
}

fn to_usize(value: &Value) -> Option<usize> {
    value.as_int().and_then(|i| usize::try_from(i).ok())
}

/// A tuple of non-negative ints.
fn dims(value: &Value) -> Option<Vec<usize>> {
    match value {
        Value::Tuple(items) => items.iter().map(to_usize).collect(),
        _ => None,
    }
}

/// `('storage', <storage class>, key, location, numel)` from BINPERSID.
fn persistent_id(pid: Item) -> Result<Item, ConvertError> {
    if let Item::Tuple(items) = &pid
        && let [
            Item::V {
                value: Value::Str(tag),
                ..
            },
            Item::Global(Allowed::Storage(dtype)),
            Item::V {
                value: Value::Str(key),
                ..
            },
            Item::V {
                value: Value::Str(_),
                ..
            },
            Item::V { value: numel, .. },
        ] = items.as_slice()
        && tag == "storage"
        && let Some(numel) = to_usize(numel)
    {
        return Ok(Item::Storage {
            dtype: *dtype,
            key: key.clone(),
            numel,
        });
    }
    Err(bad("persistent id"))
}

/// A string as `Str` if it is UTF-8, else as `Bytes`.
fn text(bytes: &[u8]) -> Value {
    match String::from_utf8(bytes.to_vec()) {
        Ok(s) => Value::Str(s),
        Err(e) => Value::Bytes(e.into_bytes()),
    }
}

/// A LONG1 payload: little-endian two's complement, at most 8 bytes.
fn long1(bytes: &[u8]) -> i64 {
    let fill = match bytes.last() {
        Some(&b) if b >= 0x80 => 0xff,
        _ => 0,
    };
    let mut buf = [fill; 8];
    for (dst, src) in buf.iter_mut().zip(bytes) {
        *dst = *src;
    }
    i64::from_le_bytes(buf)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ConvertError> {
        let end = self.pos.checked_add(n).ok_or_else(truncated)?;
        let slice = self.bytes.get(self.pos..end).ok_or_else(truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ConvertError> {
        self.take(N)?.try_into().map_err(|_| truncated())
    }

    fn u8(&mut self) -> Result<u8, ConvertError> {
        Ok(u8::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ConvertError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// The bytes of a string whose length was declared as `n`.
    fn string(&mut self, n: usize) -> Result<&'a [u8], ConvertError> {
        if n > MAX_STRING {
            return Err(limit("string"));
        }
        self.take(n)
    }

    /// The bytes up to the next newline, which is skipped.
    fn line(&mut self) -> Result<&'a [u8], ConvertError> {
        let rest = self.bytes.get(self.pos..).ok_or_else(truncated)?;
        let n = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(truncated)?;
        let line = self.take(n)?;
        self.take(1)?;
        Ok(line)
    }
}

#[derive(Default)]
struct Machine {
    stack: Vec<Item>,
    marks: Vec<usize>,
    memo: std::collections::HashMap<u32, Item>,
    /// Total cost of memo clones so far.
    cloned: usize,
}

impl Machine {
    fn run(mut self, r: &mut Cursor) -> Result<Value, ConvertError> {
        for _ in 0..MAX_OPS {
            let op = r.u8()?;
            match op {
                0x80 => {
                    let version = r.u8()?;
                    if version > 5 {
                        return Err(bad(format!("protocol {version}")));
                    }
                }
                0x95 => {
                    r.take(8)?;
                }
                b'}' => self.push(leaf(Value::Dict(Vec::new())))?,
                b']' => self.push(leaf(Value::List(Vec::new())))?,
                b')' => self.push(leaf(Value::Tuple(Vec::new())))?,
                b'(' => {
                    if self.marks.len() >= MAX_MARKS {
                        return Err(limit("marks"));
                    }
                    self.marks.push(self.stack.len());
                }
                b't' => {
                    let items = self.pop_mark()?;
                    self.push(tuple(items)?)?;
                }
                0x85..=0x87 => {
                    let n = usize::from(op - 0x84);
                    let items = self.pop_n(n)?;
                    self.push(tuple(items)?)?;
                }
                b'J' => self.push_int(i64::from(i32::from_le_bytes(r.array()?)))?,
                b'K' => self.push_int(i64::from(r.u8()?))?,
                b'M' => self.push_int(i64::from(u16::from_le_bytes(r.array()?)))?,
                0x8a => {
                    let n = usize::from(r.u8()?);
                    if n > 8 {
                        return Err(bad("long1 longer than 8 bytes"));
                    }
                    let value = long1(r.take(n)?);
                    self.push_int(value)?;
                }
                b'G' => self.push(leaf(Value::Float(f64::from_be_bytes(r.array()?))))?,
                b'X' | b'B' => {
                    let n = usize::try_from(r.u32()?).map_err(|_| limit("string"))?;
                    self.push_string(op, r.string(n)?)?;
                }
                b'T' => {
                    let n = usize::try_from(i32::from_le_bytes(r.array()?))
                        .map_err(|_| bad("negative string length"))?;
                    self.push_string(op, r.string(n)?)?;
                }
                0x8c | b'U' | b'C' => {
                    let n = usize::from(r.u8()?);
                    self.push_string(op, r.string(n)?)?;
                }
                b's' => {
                    let v = self.pop_value()?;
                    let k = self.pop_value()?;
                    self.set_items(vec![k, v])?;
                }
                b'u' => {
                    let items = self.pop_mark()?;
                    let items = items
                        .into_iter()
                        .map(into_value)
                        .collect::<Result<_, _>>()?;
                    self.set_items(items)?;
                }
                b'a' => {
                    let v = self.pop_value()?;
                    self.append(vec![v])?;
                }
                b'e' => {
                    let items = self.pop_mark()?;
                    let items = items
                        .into_iter()
                        .map(into_value)
                        .collect::<Result<_, _>>()?;
                    self.append(items)?;
                }
                b'q' => self.put(u32::from(r.u8()?))?,
                b'r' => self.put(r.u32()?)?,
                0x94 => {
                    let i = u32::try_from(self.memo.len()).map_err(|_| limit("memo"))?;
                    self.put(i)?;
                }
                b'h' => self.get(u32::from(r.u8()?))?,
                b'j' => self.get(r.u32()?)?,
                b'c' => {
                    let module = r.line()?;
                    let name = r.line()?;
                    let global = Allowed::find(module, name).ok_or_else(|| {
                        let mut what = format!(
                            "global {}.{}",
                            String::from_utf8_lossy(module),
                            String::from_utf8_lossy(name)
                        );
                        what.truncate(what.floor_char_boundary(200));
                        bad(what)
                    })?;
                    self.push(Item::Global(global))?;
                }
                b'R' => {
                    let args = self.pop()?;
                    let callable = self.pop()?;
                    self.push(reduce(callable, args)?)?;
                }
                b'b' => {
                    let state = self.pop()?;
                    let target = self.top()?;
                    match (target, state) {
                        (
                            Item::V {
                                value: Value::Dict(_),
                                ..
                            },
                            Item::V {
                                value: Value::Dict(_) | Value::None,
                                ..
                            },
                        ) => {}
                        _ => return Err(bad("build on something other than a dict")),
                    }
                }
                b'Q' => {
                    let pid = self.pop()?;
                    self.push(persistent_id(pid)?)?;
                }
                0x88 => self.push(leaf(Value::Bool(true)))?,
                0x89 => self.push(leaf(Value::Bool(false)))?,
                b'N' => self.push(leaf(Value::None))?,
                b'.' => {
                    return match self.pop()? {
                        Item::V { value, .. } => Ok(value),
                        _ => Err(bad("the result is not a value")),
                    };
                }
                _ => return Err(bad(format!("opcode 0x{op:02x}"))),
            }
        }
        Err(limit("opcodes"))
    }

    fn push(&mut self, item: Item) -> Result<(), ConvertError> {
        if self.stack.len() >= MAX_STACK {
            return Err(limit("stack"));
        }
        self.stack.push(item);
        Ok(())
    }

    fn push_int(&mut self, i: i64) -> Result<(), ConvertError> {
        self.push(leaf(Value::Int(i)))
    }

    /// BINBYTES and SHORT_BINBYTES give bytes; the other string opcodes text.
    fn push_string(&mut self, op: u8, bytes: &[u8]) -> Result<(), ConvertError> {
        let value = match op {
            b'B' | b'C' => Value::Bytes(bytes.to_vec()),
            _ => text(bytes),
        };
        self.push(leaf(value))
    }

    /// Stack length below which the innermost MARK's items start.
    fn floor(&self) -> usize {
        self.marks.last().copied().unwrap_or(0)
    }

    fn pop(&mut self) -> Result<Item, ConvertError> {
        if self.stack.len() <= self.floor() {
            return Err(bad("stack underflow"));
        }
        self.stack.pop().ok_or_else(|| bad("stack underflow"))
    }

    fn pop_value(&mut self) -> Result<(Value, usize), ConvertError> {
        into_value(self.pop()?)
    }

    fn pop_n(&mut self, n: usize) -> Result<Vec<Item>, ConvertError> {
        match self.stack.len().checked_sub(n) {
            Some(at) if at >= self.floor() => Ok(self.stack.split_off(at)),
            _ => Err(bad("stack underflow")),
        }
    }

    fn pop_mark(&mut self) -> Result<Vec<Item>, ConvertError> {
        let at = self.marks.pop().ok_or_else(|| bad("no mark"))?;
        if at > self.stack.len() {
            return Err(bad("stack underflow"));
        }
        Ok(self.stack.split_off(at))
    }

    fn top(&mut self) -> Result<&mut Item, ConvertError> {
        if self.stack.len() <= self.floor() {
            return Err(bad("stack underflow"));
        }
        self.stack.last_mut().ok_or_else(|| bad("stack underflow"))
    }

    /// SETITEM(S): `items` alternate key and value.
    fn set_items(&mut self, items: Vec<(Value, usize)>) -> Result<(), ConvertError> {
        if !items.len().is_multiple_of(2) {
            return Err(bad("odd number of dict items"));
        }
        let Item::V {
            value: Value::Dict(dict),
            depth,
        } = self.top()?
        else {
            return Err(bad("setitem on something other than a dict"));
        };
        if dict.len().saturating_add(items.len() / 2) > MAX_CONTAINER {
            return Err(limit("container"));
        }
        let mut items = items.into_iter();
        while let (Some((k, kd)), Some((v, vd))) = (items.next(), items.next()) {
            nest(depth, kd)?;
            nest(depth, vd)?;
            dict.push((k, v));
        }
        Ok(())
    }

    /// APPEND(S).
    fn append(&mut self, items: Vec<(Value, usize)>) -> Result<(), ConvertError> {
        let Item::V {
            value: Value::List(list),
            depth,
        } = self.top()?
        else {
            return Err(bad("append on something other than a list"));
        };
        if list.len().saturating_add(items.len()) > MAX_CONTAINER {
            return Err(limit("container"));
        }
        for (v, d) in items {
            nest(depth, d)?;
            list.push(v);
        }
        Ok(())
    }

    /// Stores a snapshot of the top item.
    fn put(&mut self, i: u32) -> Result<(), ConvertError> {
        if !self.memo.contains_key(&i) && self.memo.len() >= MAX_MEMO {
            return Err(limit("memo"));
        }
        if self.stack.len() <= self.floor() {
            return Err(bad("stack underflow"));
        }
        let item = self.stack.last().ok_or_else(|| bad("stack underflow"))?;
        let item = charge(&mut self.cloned, item)?.clone();
        self.memo.insert(i, item);
        Ok(())
    }

    fn get(&mut self, i: u32) -> Result<(), ConvertError> {
        let item = self.memo.get(&i).ok_or_else(|| bad("memo key missing"))?;
        let item = charge(&mut self.cloned, item)?.clone();
        self.push(item)
    }
}

/// Adds the cost of cloning `item` to `cloned`, failing past [`MAX_NODES`].
fn charge<'a>(cloned: &mut usize, item: &'a Item) -> Result<&'a Item, ConvertError> {
    *cloned = cloned.saturating_add(item_cost(item));
    if *cloned > MAX_NODES {
        return Err(limit("nodes"));
    }
    Ok(item)
}

fn into_value(item: Item) -> Result<(Value, usize), ConvertError> {
    match item {
        Item::V { value, depth } => Ok((value, depth)),
        _ => Err(bad("a global or storage used as a value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{P, contiguous, pickle};

    /// `body` between PROTO 2 and STOP.
    fn run(body: &[u8]) -> Result<Value, ConvertError> {
        let mut bytes = vec![0x80, 2];
        bytes.extend(body);
        bytes.push(b'.');
        load(&bytes)
    }

    fn unsafe_msg(result: Result<Value, ConvertError>) -> String {
        match result {
            Err(ConvertError::UnsafeContent(m)) => m,
            other => panic!("expected UnsafeContent, got {other:?}"),
        }
    }

    fn s(text: &str) -> P {
        P::Str(text.into())
    }

    fn vs(text: &str) -> Value {
        Value::Str(text.into())
    }

    fn global(module: &str, name: &str) -> P {
        P::Global(module.into(), name.into())
    }

    /// Calls `module.name(*args)` with REDUCE. A list holds the call, so the
    /// pickle stays valid; the result is the list's only item.
    fn reduce(module: &str, name: &str, args: Vec<P>) -> P {
        P::List(vec![
            global(module, name),
            P::Tuple(args),
            P::Raw(b"R".to_vec()),
        ])
    }

    fn only_item(v: Value) -> Value {
        match v {
            Value::List(mut items) if items.len() == 1 => items.remove(0),
            other => panic!("expected a one-item list, got {other:?}"),
        }
    }

    /// The persistent id `('storage', torch.<storage>, key, 'cpu', numel)` and BINPERSID.
    fn storage(tag: &str, module: &str, name: &str, key: &str, numel: u8) -> P {
        let mut b = vec![b'(', b'U', u8::try_from(tag.len()).unwrap()];
        b.extend(tag.as_bytes());
        b.push(b'c');
        b.extend(format!("{module}\n{name}\n").as_bytes());
        b.push(b'U');
        b.push(u8::try_from(key.len()).unwrap());
        b.extend(key.as_bytes());
        b.extend(b"U\x03cpu");
        b.extend([b'K', numel, b't', b'Q']);
        P::Raw(b)
    }

    fn tensor(dtype: DType, numel: usize, offset: usize, shape: &[usize]) -> P {
        P::Tensor {
            key: "0".into(),
            dtype,
            numel,
            offset,
            shape: shape.to_vec(),
            stride: contiguous(shape),
        }
    }

    fn tensor_ref(dtype: DType, numel: usize, offset: usize, shape: &[usize]) -> Value {
        Value::Tensor(TensorRef {
            key: "0".into(),
            dtype,
            storage_numel: numel,
            offset,
            shape: shape.to_vec(),
        })
    }

    #[test]
    fn loads_scalars_and_strings() {
        assert_eq!(run(b"N").unwrap(), Value::None);
        assert_eq!(run(&[0x88]).unwrap(), Value::Bool(true));
        assert_eq!(run(&[0x89]).unwrap(), Value::Bool(false));
        assert_eq!(run(b"K\xff").unwrap(), Value::Int(255));
        assert_eq!(run(b"M\x34\x12").unwrap(), Value::Int(0x1234));
        let mut binint = vec![b'J'];
        binint.extend((-5i32).to_le_bytes());
        assert_eq!(run(&binint).unwrap(), Value::Int(-5));
        assert_eq!(run(&[0x8a, 0]).unwrap(), Value::Int(0));
        assert_eq!(run(&[0x8a, 1, 0xff]).unwrap(), Value::Int(-1));
        assert_eq!(run(&[0x8a, 2, 0x00, 0x01]).unwrap(), Value::Int(256));
        assert_eq!(run(&[0x8a, 1, 0x80]).unwrap(), Value::Int(-128));
        let mut long = vec![0x8a, 8];
        long.extend(i64::MIN.to_le_bytes());
        assert_eq!(run(&long).unwrap(), Value::Int(i64::MIN));
        assert!(unsafe_msg(run(&[0x8a, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0])).contains("long"));
        let mut float = vec![b'G'];
        float.extend(1.5f64.to_be_bytes());
        assert_eq!(run(&float).unwrap(), Value::Float(1.5));

        let mut unicode = vec![b'X'];
        unicode.extend(6u32.to_le_bytes());
        unicode.extend("héllo".as_bytes());
        assert_eq!(run(&unicode).unwrap(), vs("héllo"));
        assert_eq!(run(b"\x8c\x03abc").unwrap(), vs("abc"));
        assert_eq!(run(b"U\x02hi").unwrap(), vs("hi"));
        assert_eq!(
            run(b"U\x02\xff\xfe").unwrap(),
            Value::Bytes(vec![0xff, 0xfe])
        );
        let mut binstring = vec![b'T'];
        binstring.extend(2i32.to_le_bytes());
        binstring.extend(b"ok");
        assert_eq!(run(&binstring).unwrap(), vs("ok"));
        let mut negative = vec![b'T'];
        negative.extend((-1i32).to_le_bytes());
        assert!(run(&negative).is_err());
        assert_eq!(run(b"C\x02\x00\x01").unwrap(), Value::Bytes(vec![0, 1]));
        let mut binbytes = vec![b'B'];
        binbytes.extend(3u32.to_le_bytes());
        binbytes.extend([7, 8, 9]);
        assert_eq!(run(&binbytes).unwrap(), Value::Bytes(vec![7, 8, 9]));
    }

    #[test]
    fn loads_containers() {
        let ints = |v: &[i64]| v.iter().map(|&i| Value::Int(i)).collect::<Vec<_>>();
        assert_eq!(
            run(b"]q\x00(K\x01K\x02e").unwrap(),
            Value::List(ints(&[1, 2]))
        );
        assert_eq!(run(b"]K\x01aK\x02a").unwrap(), Value::List(ints(&[1, 2])));
        assert_eq!(
            run(b"}K\x01K\x02s").unwrap(),
            Value::Dict(vec![(Value::Int(1), Value::Int(2))])
        );
        assert_eq!(
            run(b"}(K\x01K\x02K\x03K\x04u").unwrap(),
            Value::Dict(vec![
                (Value::Int(1), Value::Int(2)),
                (Value::Int(3), Value::Int(4))
            ])
        );
        assert_eq!(
            run(b"(K\x01K\x02K\x03K\x04t").unwrap(),
            Value::Tuple(ints(&[1, 2, 3, 4]))
        );
        assert_eq!(run(b"K\x01\x85").unwrap(), Value::Tuple(ints(&[1])));
        assert_eq!(run(b"K\x01K\x02\x86").unwrap(), Value::Tuple(ints(&[1, 2])));
        assert_eq!(
            run(b"K\x01K\x02K\x03\x87").unwrap(),
            Value::Tuple(ints(&[1, 2, 3]))
        );
        assert_eq!(run(b")").unwrap(), Value::Tuple(Vec::new()));

        let written = pickle(&P::Dict(vec![
            (
                s("list"),
                P::List(vec![P::Int(-7), P::Int(1 << 40), P::None]),
            ),
            (s("tuple"), P::Tuple(vec![P::Float(0.25), P::Bool(true)])),
            (s("long"), P::Tuple((0..5).map(P::Int).collect())),
            (s("bytes"), P::Bytes(vec![1, 2])),
            (s("empty"), P::List(Vec::new())),
        ]));
        assert_eq!(
            load(&written).unwrap(),
            Value::Dict(vec![
                (
                    vs("list"),
                    Value::List(vec![Value::Int(-7), Value::Int(1 << 40), Value::None])
                ),
                (
                    vs("tuple"),
                    Value::Tuple(vec![Value::Float(0.25), Value::Bool(true)])
                ),
                (vs("long"), Value::Tuple(ints(&[0, 1, 2, 3, 4]))),
                (vs("bytes"), Value::Bytes(vec![1, 2])),
                (vs("empty"), Value::List(Vec::new())),
            ])
        );
    }

    #[test]
    fn memo_get_and_put() {
        let mut body = b"(K\x07q\x03K\x08r".to_vec();
        body.extend(1000u32.to_le_bytes());
        body.extend(b"K\x09\x94h\x03j");
        body.extend(1000u32.to_le_bytes());
        // MEMOIZE stored 9 under index 2, the memo's size at the time.
        body.extend(b"h\x02t");
        assert_eq!(
            run(&body).unwrap(),
            Value::Tuple([7, 8, 9, 7, 8, 9].map(Value::Int).to_vec())
        );
        assert!(run(b"h\x05").is_err());
        assert!(run(b"q\x00").is_err());
    }

    #[test]
    fn frame_and_proto_accepted() {
        let mut bytes = vec![0x80, 4, 0x95];
        bytes.extend(3u64.to_le_bytes());
        bytes.extend(b"K\x01.");
        assert_eq!(load(&bytes).unwrap(), Value::Int(1));
        assert!(unsafe_msg(load(b"\x80\x06K\x01.")).contains("protocol"));
    }

    #[test]
    fn ordered_dict_becomes_dict() {
        let written = pickle(&P::OrderedDict(vec![
            (s("b"), P::Int(1)),
            (s("a"), P::Int(2)),
        ]));
        assert_eq!(
            load(&written).unwrap(),
            Value::Dict(vec![(vs("b"), Value::Int(1)), (vs("a"), Value::Int(2))])
        );
        let with_args = reduce("collections", "OrderedDict", vec![P::Int(1)]);
        assert!(load(&pickle(&with_args)).is_err());
    }

    #[test]
    fn rebuild_tensor_v2_and_parameter() {
        let written = pickle(&P::Dict(vec![
            (s("w"), tensor(DType::F16, 6, 0, &[2, 3])),
            (s("b"), tensor(DType::F32, 10, 4, &[3])),
            (s("scalar"), tensor(DType::I64, 1, 0, &[])),
        ]));
        assert_eq!(
            load(&written).unwrap(),
            Value::Dict(vec![
                (vs("w"), tensor_ref(DType::F16, 6, 0, &[2, 3])),
                (vs("b"), tensor_ref(DType::F32, 10, 4, &[3])),
                (vs("scalar"), tensor_ref(DType::I64, 1, 0, &[])),
            ])
        );

        // A size-1 dim may have any stride.
        let loose = P::Tensor {
            key: "0".into(),
            dtype: DType::BF16,
            numel: 3,
            offset: 0,
            shape: vec![1, 3],
            stride: vec![99, 1],
        };
        assert_eq!(
            load(&pickle(&loose)).unwrap(),
            tensor_ref(DType::BF16, 3, 0, &[1, 3])
        );

        let parameter = reduce(
            "torch._utils",
            "_rebuild_parameter",
            vec![
                tensor(DType::F32, 4, 0, &[4]),
                P::Bool(true),
                P::OrderedDict(Vec::new()),
            ],
        );
        assert_eq!(
            only_item(load(&pickle(&parameter)).unwrap()),
            tensor_ref(DType::F32, 4, 0, &[4])
        );

        let seven = |metadata: P| {
            reduce(
                "torch._utils",
                "_rebuild_tensor_v2",
                vec![
                    storage("storage", "torch", "FloatStorage", "0", 6),
                    P::Int(0),
                    P::Tuple(vec![P::Int(6)]),
                    P::Tuple(vec![P::Int(1)]),
                    P::Bool(false),
                    P::OrderedDict(Vec::new()),
                    metadata,
                ],
            )
        };
        assert_eq!(
            only_item(load(&pickle(&seven(P::None))).unwrap()),
            tensor_ref(DType::F32, 6, 0, &[6])
        );
        assert_eq!(
            only_item(load(&pickle(&seven(P::Dict(Vec::new())))).unwrap()),
            tensor_ref(DType::F32, 6, 0, &[6])
        );
        assert!(load(&pickle(&seven(P::Int(1)))).is_err());
    }

    #[test]
    fn build_on_dict_discards_state() {
        let state = P::Dict(vec![(s("_metadata"), P::Dict(Vec::new()))]);
        for state in [state, P::None] {
            let built = P::List(vec![
                P::OrderedDict(vec![(s("a"), P::Int(1))]),
                state,
                P::Raw(b"b".to_vec()),
            ]);
            assert_eq!(
                only_item(load(&pickle(&built)).unwrap()),
                Value::Dict(vec![(vs("a"), Value::Int(1))])
            );
        }
        let bad_state = P::List(vec![P::Dict(Vec::new()), P::Int(1), P::Raw(b"b".to_vec())]);
        assert!(load(&pickle(&bad_state)).is_err());
    }

    #[test]
    fn rejects_os_system() {
        let call = reduce("os", "system", vec![s("calc")]);
        assert!(unsafe_msg(load(&pickle(&call))).contains("os.system"));
    }

    #[test]
    fn rejects_unknown_opcode() {
        assert!(unsafe_msg(run(b"X\x00\x00\x00\x00X\x00\x00\x00\x00\x93")).contains("0x93"));
    }

    #[test]
    fn rejects_reduce_on_storage_global() {
        let call = reduce("torch", "FloatStorage", vec![P::Int(1)]);
        assert!(load(&pickle(&call)).is_err());
    }

    #[test]
    fn rejects_bad_persid() {
        let wrong_tag = storage("module", "torch", "FloatStorage", "0", 1);
        let not_storage = storage("storage", "collections", "OrderedDict", "0", 1);
        for pid in [wrong_tag, not_storage] {
            let call = reduce(
                "torch._utils",
                "_rebuild_tensor_v2",
                vec![
                    pid,
                    P::Int(0),
                    P::Tuple(vec![P::Int(1)]),
                    P::Tuple(vec![P::Int(1)]),
                    P::Bool(false),
                    P::OrderedDict(Vec::new()),
                ],
            );
            assert!(unsafe_msg(load(&pickle(&call))).contains("persistent id"));
        }
        assert!(unsafe_msg(run(b"K\x01Q")).contains("persistent id"));
    }

    #[test]
    fn rejects_build_on_tensor() {
        let built = P::List(vec![
            tensor(DType::F32, 1, 0, &[1]),
            P::Dict(Vec::new()),
            P::Raw(b"b".to_vec()),
        ]);
        assert!(load(&pickle(&built)).is_err());
    }

    #[test]
    fn rejects_non_contiguous_tensor() {
        let strided = |shape: Vec<usize>, stride: Vec<usize>| {
            pickle(&P::Tensor {
                key: "0".into(),
                dtype: DType::F32,
                numel: 6,
                offset: 0,
                shape,
                stride,
            })
        };
        assert!(unsafe_msg(load(&strided(vec![2, 3], vec![1, 2]))).contains("tensor"));
        assert!(unsafe_msg(load(&strided(vec![2, 3], vec![3]))).contains("tensor"));
        assert!(unsafe_msg(load(&strided(vec![1; 9], vec![1; 9]))).contains("tensor"));
    }

    #[test]
    fn rejects_tensor_outside_storage() {
        assert!(unsafe_msg(load(&pickle(&tensor(DType::F32, 5, 0, &[2, 3])))).contains("tensor"));
        assert!(unsafe_msg(load(&pickle(&tensor(DType::F32, 6, 1, &[2, 3])))).contains("tensor"));
        let huge = P::Tensor {
            key: "0".into(),
            dtype: DType::F32,
            numel: 6,
            offset: 0,
            shape: vec![1 << 62, 4],
            stride: vec![4, 1],
        };
        assert!(unsafe_msg(load(&pickle(&huge))).contains("tensor"));
        let negative = reduce(
            "torch._utils",
            "_rebuild_tensor_v2",
            vec![
                storage("storage", "torch", "FloatStorage", "0", 6),
                P::Int(-1),
                P::Tuple(vec![P::Int(1)]),
                P::Tuple(vec![P::Int(1)]),
                P::Bool(false),
                P::OrderedDict(Vec::new()),
            ],
        );
        assert!(unsafe_msg(load(&pickle(&negative))).contains("tensor"));
    }

    #[test]
    fn rejects_mark_errors() {
        assert!(run(b"}K\x01K\x02u").is_err());
        assert!(run(b"}(K\x01K\x02K\x03u").is_err());
        assert!(run(b"K\x01(\x85").is_err());
        assert!(run(b"K\x01K\x02a").is_err());
        assert!(run(b"](K\x01u").is_err());
        assert!(run(b"t").is_err());
    }

    #[test]
    fn limits_are_errors() {
        let mut long = vec![b'X'];
        long.extend(u32::try_from(MAX_STRING + 1).unwrap().to_le_bytes());
        assert!(unsafe_msg(run(&long)).contains("limit"));

        let marks = vec![b'('; MAX_MARKS + 1];
        assert!(unsafe_msg(run(&marks)).contains("limit"));

        let mut chain = b"Nq\x00".to_vec();
        for _ in 0..40 {
            chain.extend(b"h\x00h\x00\x86q\x00");
        }
        assert!(unsafe_msg(run(&chain)).contains("limit"));

        // Clones of long strings are charged by their length, not as one node.
        let mut strings = vec![b'X'];
        strings.extend(4_096u32.to_le_bytes());
        strings.extend(vec![b'a'; 4_096]);
        strings.extend(b"q\x00");
        for _ in 0..16 {
            strings.extend(b"h\x00h\x00\x86q\x00");
        }
        assert!(unsafe_msg(run(&strings)).contains("limit"));

        // Deep nesting would overflow the stack when the tree is dropped.
        let mut deep = vec![b']'; MAX_DEPTH + 1];
        deep.extend(vec![b'a'; MAX_DEPTH]);
        assert!(unsafe_msg(run(&deep)).contains("limit"));
        let mut tuples = b"N".to_vec();
        tuples.extend(vec![0x85; MAX_DEPTH]);
        assert!(unsafe_msg(run(&tuples)).contains("limit"));
        let mut ok = vec![b']'; MAX_DEPTH];
        ok.extend(vec![b'a'; MAX_DEPTH - 1]);
        assert!(run(&ok).is_ok());
    }

    #[test]
    fn truncated_input_is_error() {
        let written = pickle(&P::Dict(vec![
            (
                s("weight"),
                P::OrderedDict(vec![(s("w"), tensor(DType::F16, 6, 0, &[2, 3]))]),
            ),
            (
                s("config"),
                P::List(vec![P::Int(1025), P::Int(-3), P::Int(1 << 40), s("1")]),
            ),
            (s("ratio"), P::Float(0.5)),
            (s("raw"), P::Bytes(vec![1, 2, 3])),
            (s("f0"), P::Bool(true)),
        ]));
        assert!(load(&written).is_ok());
        for end in 0..written.len() {
            assert!(load(&written[..end]).is_err(), "prefix of {end} bytes");
        }
    }

    #[test]
    fn value_accessors() {
        let dict = Value::Dict(vec![
            (vs("a"), Value::Int(1)),
            (Value::Int(2), vs("x")),
            (vs("a"), Value::Int(3)),
            (vs("s"), vs("text")),
            (vs("l"), Value::List(vec![Value::None])),
            (vs("t"), Value::Tuple(vec![Value::Int(1), Value::Int(2)])),
        ]);
        assert_eq!(dict.get("a"), Some(&Value::Int(3)));
        assert_eq!(dict.get("missing"), None);
        assert_eq!(Value::Int(1).get("a"), None);
        assert_eq!(dict.get("s").and_then(Value::as_str), Some("text"));
        assert_eq!(Value::Int(1).as_str(), None);
        assert_eq!(dict.get("a").and_then(Value::as_int), Some(3));
        assert_eq!(vs("1").as_int(), None);
        assert_eq!(
            dict.get("l").and_then(Value::as_list).map(<[Value]>::len),
            Some(1)
        );
        assert_eq!(
            dict.get("t").and_then(Value::as_list).map(<[Value]>::len),
            Some(2)
        );
        assert_eq!(Value::None.as_list(), None);
    }
}
