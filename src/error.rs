//! Error and Result module.
use std::error::Error as StdError;
use std::fmt;
use std::io::Error as IoError;
use std::str::Utf8Error;
use std::string::FromUtf8Error;

use httparse;
#[cfg(feature = "http2")]
use h2;

pub use uri::UriError;

use self::Error::{
    Method,
    Uri,
    Version,
    Header,
    Status,
    Timeout,
    Upgrade,
    Io,
    TooLarge,
    Incomplete,
    Utf8
};

/// Result type often returned from methods that can have hyper `Error`s.
pub type Result<T> = ::std::result::Result<T, Error>;

/// A set of errors that can occur parsing HTTP streams.
#[derive(Debug)]
pub enum Error {
    /// An invalid `Method`, such as `GE,T`.
    Method,
    /// An invalid `Uri`, such as `exam ple.domain`.
    Uri(UriError),
    /// An invalid `HttpVersion`, such as `HTP/1.1`
    Version,
    /// An invalid `Header`.
    Header,
    /// A message head is too large to be reasonable.
    TooLarge,
    /// A message reached EOF, but is not complete.
    Incomplete,
    /// An invalid `Status`, such as `1337 ELITE`.
    Status,
    /// A timeout occurred waiting for an IO event.
    Timeout,
    /// A protocol upgrade was encountered, but not yet supported in hyper.
    Upgrade,
    /// An HTTP/2 error.
    #[cfg(feature = "http2")]
    Http2(Http2),
    /// An `io::Error` that occurred while trying to read or write to a network stream.
    Io(IoError),
    /// Parsing a field as string failed
    Utf8(Utf8Error),

    #[doc(hidden)]
    __Nonexhaustive(Void)
}

// Wraps an `h2::Error`. For now, this just a mostly opaque type, as we explore
// adding h2 into hyper. It can be printed for debugging purposes.
#[doc(hidden)]
#[cfg(feature = "http2")]
pub struct Http2 {
    inner: h2::Error,
}

#[cfg(feature = "http2")]
impl fmt::Debug for Http2 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

#[cfg(feature = "http2")]
impl fmt::Display for Http2 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

#[doc(hidden)]
pub struct Void(());

impl fmt::Debug for Void {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unreachable!()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Uri(ref e) => fmt::Display::fmt(e, f),
            Io(ref e) => fmt::Display::fmt(e, f),
            Utf8(ref e) => fmt::Display::fmt(e, f),
            ref e => f.write_str(e.description()),
        }
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self {
            Method => "invalid Method specified",
            Version => "invalid HTTP version specified",
            Header => "invalid Header provided",
            TooLarge => "message head is too large",
            Status => "invalid Status provided",
            Incomplete => "message is incomplete",
            Timeout => "timeout",
            Upgrade => "unsupported protocol upgrade",
            #[cfg(feature = "http2")]
            Error::Http2(ref e) => e.inner.description(),
            Uri(ref e) => e.description(),
            Io(ref e) => e.description(),
            Utf8(ref e) => e.description(),
            Error::__Nonexhaustive(..) =>  unreachable!(),
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Io(ref error) => Some(error),
            Uri(ref error) => Some(error),
            Utf8(ref error) => Some(error),
            Error::__Nonexhaustive(..) =>  unreachable!(),
            _ => None,
        }
    }
}

impl From<UriError> for Error {
    fn from(err: UriError) -> Error {
        Uri(err)
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Io(err)
    }
}

impl From<Utf8Error> for Error {
    fn from(err: Utf8Error) -> Error {
        Utf8(err)
    }
}

impl From<FromUtf8Error> for Error {
    fn from(err: FromUtf8Error) -> Error {
        Utf8(err.utf8_error())
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Error {
        match err {
            httparse::Error::HeaderName |
            httparse::Error::HeaderValue |
            httparse::Error::NewLine |
            httparse::Error::Token => Header,
            httparse::Error::Status => Status,
            httparse::Error::TooManyHeaders => TooLarge,
            httparse::Error::Version => Version,
        }
    }
}


#[cfg(feature = "http2")]
impl From<h2::Error> for Error {
    fn from(err: h2::Error) -> Error {
        Error::Http2(Http2 {
            inner: err,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::io;
    use httparse;
    use super::Error;
    use super::Error::*;

    #[test]
    fn test_cause() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.description().to_owned();
        let e = Io(orig);
        assert_eq!(e.cause().unwrap().description(), desc);
    }

    macro_rules! from {
        ($from:expr => $error:pat) => {
            match Error::from($from) {
                e @ $error => {
                    assert!(e.description().len() >= 5);
                } ,
                e => panic!("{:?}", e)
            }
        }
    }

    macro_rules! from_and_cause {
        ($from:expr => $error:pat) => {
            match Error::from($from) {
                e @ $error => {
                    let desc = e.cause().unwrap().description();
                    assert_eq!(desc, $from.description().to_owned());
                    assert_eq!(desc, e.description());
                },
                _ => panic!("{:?}", $from)
            }
        }
    }

    #[test]
    fn test_from() {

        from_and_cause!(io::Error::new(io::ErrorKind::Other, "other") => Io(..));

        from!(httparse::Error::HeaderName => Header);
        from!(httparse::Error::HeaderName => Header);
        from!(httparse::Error::HeaderValue => Header);
        from!(httparse::Error::NewLine => Header);
        from!(httparse::Error::Status => Status);
        from!(httparse::Error::Token => Header);
        from!(httparse::Error::TooManyHeaders => TooLarge);
        from!(httparse::Error::Version => Version);
    }
}
