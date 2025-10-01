use std::{fmt, marker::PhantomData};

pub use crate::primitives::*;
pub use std::{ffi::CStr, fmt::Debug, iter::Iterator};

pub fn dump_hex(buf: &[u8]) {
    let mut len = 0;
    for chunk in buf.chunks(16) {
        print!("{len:04x?}: ");
        print!("{chunk:02x?} ");
        for b in chunk {
            if b.is_ascii() && !b.is_ascii_control() {
                print!("{}", char::from_u32(*b as u32).unwrap());
            } else {
                print!(".");
            }
        }
        println!(".");
        len += chunk.len();
    }
}

pub fn dump_assert_eq(left: &[u8], right: &[u8]) {
    if left.len() != right.len() {
        dump_hex(left);
        dump_hex(right);
        panic!("Length mismatched");
    }
    if let Some(pos) = left.iter().zip(right.iter()).position(|(l, r)| *l != *r) {
        println!();
        println!("Left:");
        dump_hex(left);
        println!();
        println!("Right:");
        dump_hex(right);
        panic!("Differ at byte {pos} (0x{pos:x?})");
    }
}

pub struct FormatHex<'a>(pub &'a [u8]);
impl Debug for FormatHex<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "\"")?;
        for i in self.0 {
            write!(fmt, "{i:02x}")?
        }
        write!(fmt, "\"")?;
        Ok(())
    }
}

pub struct DisplayAsDebug<T>(T);
impl<T: fmt::Display> fmt::Debug for DisplayAsDebug<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorReason {
    /// Only used in `.get_<attr>()` methods
    AttrMissing,
    /// Value of the attribute can't be parsed
    ParsingError,
    /// Found attribute of type not mentioned in the specification
    UnknownAttr,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ErrorContext {
    pub attrs: &'static str,
    pub attr: Option<&'static str>,
    pub offset: usize,
    pub reason: ErrorReason,
}

impl std::error::Error for ErrorContext {}

impl fmt::Debug for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorContext")
            .field("message", &DisplayAsDebug(&self))
            .field("reason", &self.reason)
            .field("attrs", &self.attrs)
            .field("attr", &self.attr)
            .field("offset", &self.offset)
            .finish()
    }
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let attrs = self.attrs;
        if matches!(self.reason, ErrorReason::AttrMissing) {
            let attr = self.attr.unwrap();
            write!(f, "Missing attribute {attr:?} in {attrs:?}")?;
            return Ok(());
        } else {
            write!(f, "Error parsing ")?;
            if let Some(attr) = self.attr {
                write!(f, "attribute {attr:?} of {attrs:?}")?;
            } else {
                write!(f, "header of {attrs:?}")?;
                if matches!(self.reason, ErrorReason::UnknownAttr) {
                    write!(f, " (unknown attribute)")?;
                }
            }
        }
        write!(f, " at offset {}", self.offset)?;
        Ok(())
    }
}

pub struct FlattenErrorContext<T: fmt::Debug>(pub Result<T, ErrorContext>);

impl<T: Debug> fmt::Debug for FlattenErrorContext<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Ok(ok) => ok.fmt(f),
            Err(err) => {
                f.write_str("Err(")?;
                err.fmt(f)?;
                f.write_str(")")
            }
        }
    }
}

pub const NLA_F_NESTED: u16 = 1 << 15;
pub const NLA_F_NET_BYTEORDER: u16 = 1 << 14;

pub const fn nla_type(r#type: u16) -> u16 {
    r#type & (!(NLA_F_NESTED | NLA_F_NET_BYTEORDER))
}

pub const NLA_ALIGNTO: usize = 4;

pub const fn nla_align_up(len: usize) -> usize {
    ((len) + NLA_ALIGNTO - 1) & !(NLA_ALIGNTO - 1)
}

pub fn align(buf: &mut Vec<u8>) {
    let len = buf.len();
    buf.extend(std::iter::repeat_n(0u8, nla_align_up(len) - len));
}

/// Returns header offset
pub fn push_nested_header(buf: &mut Vec<u8>, r#type: u16) -> usize {
    push_header_type(buf, r#type, 0, true)
}

/// Returns header offset
pub fn push_header(buf: &mut Vec<u8>, r#type: u16, len: u16) -> usize {
    push_header_type(buf, r#type, len, false)
}

/// Returns header offset
/// The kernel doesn't really check byteorder bit nor set it correctly
fn push_header_type(buf: &mut Vec<u8>, mut r#type: u16, len: u16, is_nested: bool) -> usize {
    align(buf);

    let header_offset = buf.len();

    if is_nested {
        r#type |= NLA_F_NESTED;
    }

    // TODO: alignment for 8 byte types?
    buf.extend((len + 4).to_ne_bytes());
    buf.extend(r#type.to_ne_bytes());

    align(buf);

    header_offset
}

pub fn finalize_nested_header(buf: &mut Vec<u8>, offset: usize) {
    align(buf);

    let len = (buf.len() - offset) as u16;
    buf[offset..(offset + 2)].copy_from_slice(&len.to_ne_bytes());
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub r#type: u16,
    pub is_nested: bool,
}

pub fn chop_header<'a>(buf: &'a [u8], pos: &mut usize) -> Option<(Header, &'a [u8])> {
    let buf = &buf[*pos..];

    if buf.len() < 4 {
        return None;
    }

    let len = parse_u16(&buf[0..2]).unwrap();
    let r#type = parse_u16(&buf[2..4]).unwrap();

    let next_len = nla_align_up(len as usize);

    if len < 4 || buf.len() < len as usize {
        return None;
    }

    let next = &buf[4..len as usize];
    *pos += next_len.min(buf.len());

    Some((
        Header {
            r#type: nla_type(r#type),
            is_nested: r#type & NLA_F_NESTED != 0,
        },
        next,
    ))
}

pub trait Rec {
    fn as_rec_mut(&mut self) -> &mut Vec<u8>;
}

impl Rec for &mut Vec<u8> {
    fn as_rec_mut(&mut self) -> &mut Vec<u8> {
        self
    }
}

#[derive(Clone)]
pub struct Iterable<'a, AttrSet> {
    pub(crate) buf: &'a [u8],
    /// Current position of the iterable in the [`buf`]
    pub(crate) pos: usize,
    /// Pointer to the beginning of the first slice in the chain.
    /// Only used in calculating byte offset for error context.
    pub(crate) orig_loc: usize,
    pub(crate) phantom: PhantomData<AttrSet>,
}

impl<AttrSet: Clone> Copy for Iterable<'_, AttrSet> {}

impl<AttrSet> Default for Iterable<'_, AttrSet> {
    fn default() -> Self {
        Self {
            buf: &[],
            pos: 0,
            orig_loc: 0,
            phantom: PhantomData,
        }
    }
}

impl<'a, AttrSet> Iterable<'a, AttrSet> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Iterable::with_loc(buf, buf.as_ptr() as usize)
    }

    pub(crate) fn with_loc(buf: &'a [u8], orig_loc: usize) -> Self {
        Iterable {
            buf,
            pos: 0,
            orig_loc,
            phantom: PhantomData,
        }
    }

    pub(crate) fn calc_offset(&self, loc: usize) -> usize {
        let orig = self.orig_loc;

        if orig <= loc && loc - orig <= u16::MAX as usize {
            loc - orig
        } else {
            0
        }
    }

    #[cold]
    pub(crate) fn error_missing(&self, attrs: &'static str, attr: &'static str) -> ErrorContext {
        let ctx = ErrorContext {
            attrs,
            attr: Some(attr),
            offset: self.calc_offset(self.buf.as_ptr() as usize),
            reason: ErrorReason::AttrMissing,
        };

        if cfg!(test) {
            panic!("{ctx}")
        } else {
            ctx
        }
    }

    #[cold]
    pub(crate) fn error_context(
        &mut self,
        attrs: &'static str,
        attr: Option<&'static str>,
        loc: *const u8,
    ) -> ErrorContext {
        self.buf = &[];

        let ctx = ErrorContext {
            attrs,
            attr,
            offset: self.calc_offset(loc as usize),
            reason: if attr.is_some() {
                ErrorReason::ParsingError
            } else {
                ErrorReason::UnknownAttr
            },
        };

        if cfg!(test) {
            panic!("{ctx}")
        } else {
            ctx
        }
    }

    pub fn get_buf(&self) -> &'a [u8] {
        self.buf
    }
}

#[derive(Clone)]
pub struct MultiAttrIterable<I, T, V>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    pub(crate) inner: I,
    pub(crate) f: fn(T) -> Option<V>,
}

impl<I, T, V> MultiAttrIterable<I, T, V>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    pub fn new(inner: I, f: fn(T) -> Option<V>) -> Self {
        Self { inner, f }
    }
}

impl<I, T, V> Iterator for MultiAttrIterable<I, T, V>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    type Item = V;
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next() {
            Some(Ok(val)) => (self.f)(val),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct ArrayIterable<I, T>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    pub(crate) inner: I,
}

impl<I, T> ArrayIterable<I, T>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    pub fn new(inner: I) -> Self {
        Self { inner }
    }
}

impl<I, T> Iterator for ArrayIterable<I, T>
where
    I: Iterator<Item = Result<T, ErrorContext>>,
{
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next() {
            Some(Ok(val)) => Some(val),
            _ => None,
        }
    }
}
