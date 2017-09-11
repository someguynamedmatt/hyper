//! Pieces pertaining to the HTTP message protocol.
use std::borrow::Cow;
use std::fmt;

use bytes::BytesMut;
use http::header::{HeaderMap, HeaderValue};
use http::header::{CONNECTION, EXPECT};
use http::version::{HTTP_10, HTTP_11};


use {Method, StatusCode, Uri, Version};

pub use self::conn::{Conn, KeepAlive, KA};
pub use self::body::{Body, TokioBody};
pub use self::chunk::Chunk;

mod body;
mod chunk;
mod conn;
mod io;
mod h1;
//mod h2;

type Headers = HeaderMap<HeaderValue>;

/*
/// The raw status code and reason-phrase.
#[derive(Clone, PartialEq, Debug)]
pub struct RawStatus(pub u16, pub Cow<'static, str>);

impl RawStatus {
    /// Converts this into a StatusCode.
    #[inline]
    pub fn status(&self) -> StatusCode {
        StatusCode::try_from(self.0).unwrap()
    }
}

impl fmt::Display for RawStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.0, self.1)
    }
}

impl From<StatusCode> for RawStatus {
    fn from(status: StatusCode) -> RawStatus {
        RawStatus(status.into(), Cow::Borrowed(status.canonical_reason().unwrap_or("")))
    }
}

impl Default for RawStatus {
    fn default() -> RawStatus {
        RawStatus(200, Cow::Borrowed("OK"))
    }
}

impl From<MessageHead<::StatusCode>> for MessageHead<RawStatus> {
    fn from(head: MessageHead<::StatusCode>) -> MessageHead<RawStatus> {
        MessageHead {
            subject: head.subject.into(),
            version: head.version,
            headers: head.headers,
        }
    }
}
*/

/// Checks if a connection should be kept alive.
#[inline]
fn should_keep_alive(version: Version, headers: &Headers) -> bool {
    //TODO: must split the HeaderValue by commas, and use unicase::eq
    let ret = match (version, headers.get(&CONNECTION)) {
        (HTTP_10, None) => false,
        (HTTP_10, Some(val)) if val.as_bytes() != b"keep-alive" => false,
        (HTTP_11, Some(val)) if val.as_bytes() == b"close" => false,
        _ => true
    };
    trace!("should_keep_alive() = {:?}", ret);
    ret
}

/// Checks if a connection is expecting a `100 Continue` before sending its body.
#[inline]
fn expecting_continue(version: Version, headers: &Headers) -> bool {
    let ret = match (version, headers.get(&EXPECT).map(|v| v.as_bytes())) {
        (HTTP_11, Some(b"100-continue")) => true,
        _ => false
    };
    trace!("expecting_continue() = {:?}", ret);
    ret
}

#[derive(Debug)]
pub enum ServerTransaction {}

#[derive(Debug)]
pub enum ClientTransaction {}

pub trait Http1Transaction {
    type Incoming: Head;
    type Outgoing: Head;
    fn parse(bytes: &mut BytesMut) -> ParseResult<Self::Incoming>;
    fn decoder(head: &Self::Incoming, method: &mut Option<::Method>) -> ::Result<h1::Decoder>;
    fn encode(head: Self::Outgoing, has_body: bool, method: &mut Option<Method>, dst: &mut Vec<u8>) -> h1::Encoder;
}

pub type ParseResult<T> = ::Result<Option<(T, usize)>>;

struct DebugTruncate<'a>(&'a [u8]);

impl<'a> fmt::Debug for DebugTruncate<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let bytes = self.0;
        if bytes.len() > 32 {
            try!(f.write_str("["));
            for byte in &bytes[..32] {
                try!(write!(f, "{:?}, ", byte));
            }
            write!(f, "... {}]", bytes.len())
        } else {
            fmt::Debug::fmt(bytes, f)
        }
    }
}

pub trait Head {
    fn expects_continue(&self) -> bool;
    fn keep_alive(&self) -> bool;
}

impl Head for ::http::request::Parts {
    fn expects_continue(&self) -> bool {
        expecting_continue(self.version, &self.headers)
    }

    fn keep_alive(&self) -> bool {
        should_keep_alive(self.version, &self.headers)
    }
}

impl Head for ::http::response::Parts {
    fn expects_continue(&self) -> bool {
        expecting_continue(self.version, &self.headers)
    }

    fn keep_alive(&self) -> bool {
        should_keep_alive(self.version, &self.headers)
    }
}

#[test]
fn test_should_keep_alive() {
    let mut headers = Headers::new();

    assert!(!should_keep_alive(HTTP_10, &headers));
    assert!(should_keep_alive(HTTP_11, &headers));

    headers.insert(CONNECTION, HeaderValue::from_static("close"));
    assert!(!should_keep_alive(HTTP_10, &headers));
    assert!(!should_keep_alive(HTTP_11, &headers));

    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
    assert!(should_keep_alive(HTTP_10, &headers));
    assert!(should_keep_alive(HTTP_11, &headers));
}

#[test]
fn test_expecting_continue() {
    let mut headers = Headers::new();

    assert!(!expecting_continue(HTTP_10, &headers));
    assert!(!expecting_continue(HTTP_11, &headers));

    headers.insert(EXPECT, HeaderValue::from_static("100-continue"));
    assert!(!expecting_continue(HTTP_10, &headers));
    assert!(expecting_continue(HTTP_11, &headers));
}
