use std::borrow::Cow;
use std::fmt::{self, Write};

use bytes::{BytesMut, Bytes};
use http::header::{GetAll, HeaderMap, HeaderName, HeaderValue};
use http::header::{CONTENT_LENGTH, DATE, TRANSFER_ENCODING};

use http::version::{HTTP_10, HTTP_11};
use httparse;

//use header::{self, Headers, ContentLength, TransferEncoding};
use proto::{Http1Transaction, ParseResult, ServerTransaction, ClientTransaction};
use super::{Encoder, Decoder, date};

use {Method, StatusCode, Uri, Version};

const MAX_HEADERS: usize = 100;
const AVERAGE_HEADER_SIZE: usize = 30; // totally scientific

impl Http1Transaction for ServerTransaction {
    type Incoming = ::http::request::Parts;
    type Outgoing = ::http::response::Parts;

    fn parse(buf: &mut BytesMut) -> ParseResult<Self::Incoming> {
        if buf.len() == 0 {
            return Ok(None);
        }
        let mut headers_indices = [HeaderIndices {
            name: (0, 0),
            value: (0, 0)
        }; MAX_HEADERS];
        let (len, method, path, version, headers_len) = {
            let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
            trace!("Request.parse([Header; {}], [u8; {}])", headers.len(), buf.len());
            let mut req = httparse::Request::new(&mut headers);
            match try!(req.parse(&buf)) {
                httparse::Status::Complete(len) => {
                    trace!("httparse Complete({})", len);
                    let method = Method::from_bytes(req.method.unwrap().as_bytes())?;
                    let path = req.path.unwrap();
                    let bytes_ptr = buf.as_ref().as_ptr() as usize;
                    let path_start = path.as_ptr() as usize - bytes_ptr;
                    let path_end = path_start + path.len();
                    let path = (path_start, path_end);
                    let version = if req.version.unwrap() == 1 { HTTP_11 } else { HTTP_10 };

                    record_header_indices(buf.as_ref(), &req.headers, &mut headers_indices);
                    let headers_len = req.headers.len();
                    (len, method, path, version, headers_len)
                }
                httparse::Status::Partial => return Ok(None),
            }
        };

        let mut headers = HeaderMap::with_capacity(headers_len);
        let slice = buf.split_to(len).freeze();
        let path = slice.slice(path.0, path.1);
        let uri = Uri::from_shared(path)?;

        headers.extend(HeadersAsBytesIter {
            headers: headers_indices[..headers_len].iter(),
            slice: slice,
        });

        let mut head = ::http::Request::new(()).into_parts().0;
        head.method = method;
        head.uri = uri;
        head.version = version;
        head.headers = headers;

        Ok(Some((head, len)))
    }

    fn decoder(head: &Self::Incoming, method: &mut Option<Method>) -> ::Result<Decoder> {
        *method = Some(head.method.clone());

        // According to https://tools.ietf.org/html/rfc7230#section-3.3.3
        // 1. (irrelevant to Request)
        // 2. (irrelevant to Request)
        // 3. Transfer-Encoding: chunked has a chunked body.
        // 4. If multiple differing Content-Length headers or invalid, close connection.
        // 5. Content-Length header has a sized body.
        // 6. Length 0.
        // 7. (irrelevant to Request)

        if let Some(values) = head.headers.get_all(&TRANSFER_ENCODING) {
            // https://tools.ietf.org/html/rfc7230#section-3.3.3
            // If Transfer-Encoding header is present, and 'chunked' is
            // not the final encoding, and this is a Request, then it is
            // mal-formed. A server should respond with 400 Bad Request.
            let last = values.iter()
                .next_back()
                .expect("get_all always has at least 1 item");
            if find_chunked(last) {
                Ok(Decoder::chunked())
            } else {
                debug!("request with transfer-encoding header, but not chunked, bad request");
                Err(::Error::Header)
            }
        } else if let Some(values) = head.headers.get_all(&CONTENT_LENGTH) {
            match parse_content_length(values) {
                Ok(len) => {
                    Ok(Decoder::length(len))
                }
                Err(_) => {
                    //debug!("illegal Content-Length: {:?}", head.headers.get_raw("Content-Length"));
                    Err(::Error::Header)
                }
            }
        } else {
            Ok(Decoder::length(0))
        }
    }

    fn encode(mut head: Self::Outgoing, has_body: bool, method: &mut Option<Method>, dst: &mut Vec<u8>) -> Encoder {
        trace!("ServerTransaction::encode has_body={}, method={:?}", has_body, method);

        let body = ServerTransaction::set_length(
            &mut head,
            has_body,
            method.as_ref().expect("server response lost request method")
        );

        let init_cap = 30 + head.headers.len() * AVERAGE_HEADER_SIZE;
        dst.reserve(init_cap);
        if head.version == HTTP_11 && head.status == ::http::status::OK {
            // optimize common response
            extend(dst, b"HTTP/1.1 200 OK\r\n");
        } else {
            if head.version == HTTP_11 {
                extend(dst, b"HTTP/1.1 ");
            } else if head.version == HTTP_10 {
                extend(dst, b"HTTP/1.1 ");
            } else {
                panic!("unexpected version {:?}", head.version);
            }
            let _ = write!(FastWrite(dst), "{}", head.status);
            extend(dst, b"\r\n");
        }
        write_headers(dst, &head.headers);
        // using http::h1::date is quite a lot faster than generating a unique Date header each time
        // like req/s goes up about 10%
        if !head.headers.contains_key(&DATE) {
            dst.reserve(date::DATE_VALUE_LENGTH + 8);
            extend(dst, b"Date: ");
            date::extend(dst);
            extend(dst, b"\r\n");
        }
        extend(dst, b"\r\n");
        body
    }
}

impl ServerTransaction {
    fn set_length(head: &mut ::http::response::Parts, has_body: bool, method: &Method) -> Encoder {
        use http::method::{CONNECT, HEAD};
        use http::status;

        let can_have_body = {
            if method == &HEAD {
                false
            } else if method == &CONNECT && head.status.is_success() {
                false
            } else {
                match head.status {
                    // TODO: support for 1xx codes needs improvement everywhere
                    // would be 100...199 => false
                    status::NO_CONTENT |
                    status::NOT_MODIFIED => false,
                    _ => true,
                }
            }
        };

        if has_body && can_have_body {
            encode_length_or_chunked(&mut head.headers)
        } else {
            head.headers.remove(&TRANSFER_ENCODING);
            if can_have_body {
                head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
            }
            Encoder::length(0)
        }
    }
}

impl Http1Transaction for ClientTransaction {
    type Incoming = ::http::response::Parts;
    type Outgoing = ::http::request::Parts;

    fn parse(buf: &mut BytesMut) -> ParseResult<Self::Incoming> {
        if buf.len() == 0 {
            return Ok(None);
        }
        let mut headers_indices = [HeaderIndices {
            name: (0, 0),
            value: (0, 0)
        }; MAX_HEADERS];
        let (len, code, reason, version, headers_len) = {
            let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
            trace!("Response.parse([Header; {}], [u8; {}])", headers.len(), buf.len());
            let mut res = httparse::Response::new(&mut headers);
            let bytes = buf.as_ref();
            match try!(res.parse(bytes)) {
                httparse::Status::Complete(len) => {
                    trace!("Response.parse Complete({})", len);
                    let code = res.code.unwrap();
                    let status = StatusCode::from_u16(code)?;
                    let reason = match status.canonical_reason() {
                        Some(reason) if reason == res.reason.unwrap() => Cow::Borrowed(reason),
                        _ => Cow::Owned(res.reason.unwrap().to_owned())
                    };
                    let version = if res.version.unwrap() == 1 { HTTP_11 } else { HTTP_10 };
                    record_header_indices(bytes, &res.headers, &mut headers_indices);
                    let headers_len = res.headers.len();
                    (len, code, reason, version, headers_len)
                },
                httparse::Status::Partial => return Ok(None),
            }
        };

        let mut headers = HeaderMap::with_capacity(headers_len);
        let slice = buf.split_to(len).freeze();
        headers.extend(HeadersAsBytesIter {
            headers: headers_indices[..headers_len].iter(),
            slice: slice,
        });
        let mut head = ::http::Response::new(()).into_parts().0;
        head.status = StatusCode::from_u16(code)?;
        head.version = version;
        head.headers = headers;
        Ok(Some((head, len)))
    }

    fn decoder(inc: &Self::Incoming, method: &mut Option<Method>) -> ::Result<Decoder> {
        use http::method::{CONNECT, HEAD};

        // According to https://tools.ietf.org/html/rfc7230#section-3.3.3
        // 1. HEAD responses, and Status 1xx, 204, and 304 cannot have a body.
        // 2. Status 2xx to a CONNECT cannot have a body.
        // 3. Transfer-Encoding: chunked has a chunked body.
        // 4. If multiple differing Content-Length headers or invalid, close connection.
        // 5. Content-Length header has a sized body.
        // 6. (irrelevant to Response)
        // 7. Read till EOF.

        match *method {
            Some(HEAD) => {
                return Ok(Decoder::length(0));
            }
            Some(CONNECT) => match inc.status.as_u16() {
                200...299 => {
                    return Ok(Decoder::length(0));
                },
                _ => {},
            },
            Some(_) => {},
            None => {
                trace!("ClientTransaction::decoder is missing the Method");
            }
        }

        match inc.status.as_u16() {
            100...199 |
            204 |
            304 => return Ok(Decoder::length(0)),
            _ => (),
        }

        unimplemented!("decoder");
        /*
        if let Some(&header::TransferEncoding(ref codings)) = inc.headers.get() {
            if codings.last() == Some(&header::Encoding::Chunked) {
                Ok(Decoder::chunked())
            } else {
                trace!("not chunked. read till eof");
                Ok(Decoder::eof())
            }
        } else if let Some(&header::ContentLength(len)) = inc.headers.get() {
            Ok(Decoder::length(len))
        } else if inc.headers.has::<header::ContentLength>() {
            debug!("illegal Content-Length: {:?}", inc.headers.get_raw("Content-Length"));
            Err(::Error::Header)
        } else {
            trace!("neither Transfer-Encoding nor Content-Length");
            Ok(Decoder::eof())
        }
        */
    }

    fn encode(mut head: Self::Outgoing, has_body: bool, method: &mut Option<Method>, dst: &mut Vec<u8>) -> Encoder {
        *method = Some(head.method.clone());

        let body = ClientTransaction::set_length(&mut head, has_body);

        let init_cap = 30 + head.headers.len() * AVERAGE_HEADER_SIZE;
        dst.reserve(init_cap);

        dst.extend(head.method.as_str().as_bytes());
        dst.push(b' ');

        let _ = write!(FastWrite(dst), "{}", head.uri);
        dst.push(b' ');

        dst.extend(b"HTTP/1.1\r\n");

        write_headers(dst, &head.headers);
        dst.extend(b"\r\n");

        body
    }
}

impl ClientTransaction {
    fn set_length(head: &mut ::http::request::Parts, has_body: bool) -> Encoder {
        if has_body {
            encode_length_or_chunked(&mut head.headers)
        } else {
            head.headers.remove(&CONTENT_LENGTH);
            head.headers.remove(&TRANSFER_ENCODING);
            Encoder::length(0)
        }
    }
}

fn write_headers(dst: &mut Vec<u8>, headers: &HeaderMap<HeaderValue>) {
    for (name, value) in headers {
        extend(dst, name.as_str().as_bytes());
        extend(dst, b": ");
        extend(dst, value.as_bytes());
        extend(dst, b"\r\n");
    }
}

fn encode_length_or_chunked(headers: &mut HeaderMap<HeaderValue>) -> Encoder {
    let len = headers.get(&CONTENT_LENGTH).and_then(|value| {
        value.to_str().ok().and_then(|s| {
            s.parse().ok()
        })
    });

    if let Some(len) = len {
        headers.remove(&TRANSFER_ENCODING);
        Encoder::length(len)
    } else {
        use ::http::header::Entry;
        match headers.entry(TRANSFER_ENCODING).expect("constant is valid") {
            Entry::Occupied(mut entry) => {
                let last = entry.iter_mut()
                    .next_back()
                    .expect("occupied entry always has at least 1 value");

                let has_chunked = find_chunked(last);
                if !has_chunked {
                    let mut new = BytesMut::from(last.as_bytes());
                    new.extend(b", chunked");
                    // TODO: use HeaderValue::from_shared_unchecked
                    *last = HeaderValue::from_shared(new.freeze())
                        .expect("all pushed bytes are valid");
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(HeaderValue::from_static("chunked"));
            }
        }

        Encoder::chunked()
    }
}

fn parse_content_length<'a>(values: GetAll<'a, HeaderValue>) -> Result<u64, InvalidContentLength> {
    let parse = |val: &HeaderValue| {
        let s = val.to_str().map_err(|_| InvalidContentLength)?;
        s.parse().map_err(|_| InvalidContentLength)
    };
    let mut prev = None;
    for value in values {
        let len = parse(value)?;
        if let Some(p) = prev {
            if p != len {
                debug!("multiple Content-Length headers, differing values: {} vs {}", p, len);
                return Err(InvalidContentLength);
            }
        } else {
            prev = Some(len);
        }
    }
    Ok(prev.expect("GetAll always has at least 1 value"))
}

fn find_chunked(value: &HeaderValue) -> bool {
    value.to_str().map(|s| {
        let last = s.rsplit(',')
            .next()
            .expect("split always has at least 1 item")
            .trim();
        ::unicase::eq_ascii(last, "chunked")
    })
        .unwrap_or(false)
}

#[derive(Copy, Clone, Debug)]
struct InvalidContentLength;

#[derive(Clone, Copy)]
struct HeaderIndices {
    name: (usize, usize),
    value: (usize, usize),
}

fn record_header_indices(bytes: &[u8], headers: &[httparse::Header], indices: &mut [HeaderIndices]) {
    let bytes_ptr = bytes.as_ptr() as usize;
    for (header, indices) in headers.iter().zip(indices.iter_mut()) {
        let name_start = header.name.as_ptr() as usize - bytes_ptr;
        let name_end = name_start + header.name.len();
        indices.name = (name_start, name_end);
        let value_start = header.value.as_ptr() as usize - bytes_ptr;
        let value_end = value_start + header.value.len();
        indices.value = (value_start, value_end);
    }
}

struct HeadersAsBytesIter<'a> {
    headers: ::std::slice::Iter<'a, HeaderIndices>,
    slice: Bytes,
}

impl<'a> Iterator for HeadersAsBytesIter<'a> {
    type Item = (HeaderName, HeaderValue);
    fn next(&mut self) -> Option<Self::Item> {
        self.headers.next().map(|header| {
            let name = unsafe {
                let bytes = ::std::slice::from_raw_parts(
                    self.slice.as_ref().as_ptr().offset(header.name.0 as isize),
                    header.name.1 - header.name.0
                );
                // TODO: use HeaderName::from_bytes_unchecked
                HeaderName::from_bytes(bytes)
                    .expect("httparse enforces valid header names")
            };
            let bytes = self.slice.slice(header.value.0, header.value.1);
            // TODO: use HeaderValue::from_shared_unchecked
            let value = HeaderValue::from_shared(bytes)
                .expect("httparse enforces valid header values");
            (name, value)
        })
    }
}

struct FastWrite<'a>(&'a mut Vec<u8>);

impl<'a> fmt::Write for FastWrite<'a> {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        extend(self.0, s.as_bytes());
        Ok(())
    }

    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments) -> fmt::Result {
        fmt::write(self, args)
    }
}

#[inline]
fn extend(dst: &mut Vec<u8>, data: &[u8]) {
    dst.extend_from_slice(data);
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use http::header::*;
    use http::status;
    use http::version::HTTP_11;
    use http;

    use proto::{ServerTransaction, ClientTransaction, Http1Transaction};

    #[test]
    fn test_parse_request() {
        extern crate pretty_env_logger;
        let _ = pretty_env_logger::init();
        let mut raw = BytesMut::from(b"GET /echo HTTP/1.1\r\nHost: hyper.rs\r\n\r\n".to_vec());
        let expected_len = raw.len();
        let (req, len) = ServerTransaction::parse(&mut raw).unwrap().unwrap();
        assert_eq!(len, expected_len);
        assert_eq!(req.method, http::method::GET);
        assert_eq!(req.uri, "/echo");
        assert_eq!(req.version, HTTP_11);
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.headers.get("Host").map(|v| v.as_bytes()), Some(b"hyper.rs".as_ref()));
    }


    #[test]
    fn test_parse_response() {
        extern crate pretty_env_logger;
        let _ = pretty_env_logger::init();
        let mut raw = BytesMut::from(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
        let expected_len = raw.len();
        let (req, len) = ClientTransaction::parse(&mut raw).unwrap().unwrap();
        assert_eq!(len, expected_len);
        assert_eq!(req.status.as_u16(), 200);
        assert_eq!(req.version, HTTP_11);
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.headers.get("Content-Length").map(|v| v.as_bytes()), Some(b"0".as_ref()));
    }

    #[test]
    fn test_parse_request_errors() {
        let mut raw = BytesMut::from(b"GET htt:p// HTTP/1.1\r\nHost: hyper.rs\r\n\r\n".to_vec());
        ServerTransaction::parse(&mut raw).unwrap_err();
    }

    /*
    #[test]
    fn test_parse_raw_status() {
        let mut raw = BytesMut::from(b"HTTP/1.1 200 OK\r\n\r\n".to_vec());
        let (res, _) = ClientTransaction::parse(&mut raw).unwrap().unwrap();
        assert_eq!(res.subject.1, "OK");

        let mut raw = BytesMut::from(b"HTTP/1.1 200 Howdy\r\n\r\n".to_vec());
        let (res, _) = ClientTransaction::parse(&mut raw).unwrap().unwrap();
        assert_eq!(res.subject.1, "Howdy");
    }
    */


    #[test]
    fn test_decoder_request() {
        use super::Decoder;

        let method = &mut None;
        let mut head = http::Request::new(()).into_parts().0;

        head.method = http::method::GET;
        assert_eq!(Decoder::length(0), ServerTransaction::decoder(&head, method).unwrap());
        assert_eq!(*method, Some(http::method::GET));

        head.method = http::method::POST;
        assert_eq!(Decoder::length(0), ServerTransaction::decoder(&head, method).unwrap());
        assert_eq!(*method, Some(http::method::POST));

        head.headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        assert_eq!(Decoder::chunked(), ServerTransaction::decoder(&head, method).unwrap());
        // transfer-encoding and content-length = chunked
        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("10"));
        assert_eq!(Decoder::chunked(), ServerTransaction::decoder(&head, method).unwrap());

        head.headers.remove(&TRANSFER_ENCODING);
        assert_eq!(Decoder::length(10), ServerTransaction::decoder(&head, method).unwrap());

        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("5"));
        head.headers.append(CONTENT_LENGTH, HeaderValue::from_static("5"));
        assert_eq!(Decoder::length(5), ServerTransaction::decoder(&head, method).unwrap());

        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("10"));
        head.headers.append(CONTENT_LENGTH, HeaderValue::from_static("11"));
        ServerTransaction::decoder(&head, method).unwrap_err();

        head.headers.remove(&CONTENT_LENGTH);

        head.headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("notchunked"));
        ServerTransaction::decoder(&head, method).unwrap_err();
    }

    #[test]
    fn test_decoder_response() {
        use super::Decoder;

        let method = &mut Some(http::method::GET);
        let mut head = ::http::Response::new(()).into_parts().0;

        head.status = status::NO_CONTENT;
        assert_eq!(Decoder::length(0), ClientTransaction::decoder(&head, method).unwrap());
        head.status = status::NOT_MODIFIED;
        assert_eq!(Decoder::length(0), ClientTransaction::decoder(&head, method).unwrap());

        head.status = status::OK;
        assert_eq!(Decoder::eof(), ClientTransaction::decoder(&head, method).unwrap());

        *method = Some(http::method::HEAD);
        assert_eq!(Decoder::length(0), ClientTransaction::decoder(&head, method).unwrap());

        *method = Some(http::method::CONNECT);
        assert_eq!(Decoder::length(0), ClientTransaction::decoder(&head, method).unwrap());


        // CONNECT receiving non 200 can have a body
        head.status = status::NOT_FOUND;
        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("10"));
        assert_eq!(Decoder::length(10), ClientTransaction::decoder(&head, method).unwrap());
        head.headers.remove(&CONTENT_LENGTH);


        *method = Some(http::method::GET);
        head.headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        assert_eq!(Decoder::chunked(), ClientTransaction::decoder(&head, method).unwrap());

        // transfer-encoding and content-length = chunked
        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("10"));
        assert_eq!(Decoder::chunked(), ClientTransaction::decoder(&head, method).unwrap());

        head.headers.remove(&TRANSFER_ENCODING);
        assert_eq!(Decoder::length(10), ClientTransaction::decoder(&head, method).unwrap());

        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("5"));
        head.headers.append(CONTENT_LENGTH, HeaderValue::from_static("5"));
        assert_eq!(Decoder::length(5), ClientTransaction::decoder(&head, method).unwrap());

        head.headers.insert(CONTENT_LENGTH, HeaderValue::from_static("10"));
        head.headers.append(CONTENT_LENGTH, HeaderValue::from_static("11"));
        ClientTransaction::decoder(&head, method).unwrap_err();
    }

    #[cfg(feature = "nightly")]
    use test::Bencher;

    #[cfg(feature = "nightly")]
    #[bench]
    fn bench_parse_incoming(b: &mut Bencher) {
        let mut raw = BytesMut::from(
            b"GET /super_long_uri/and_whatever?what_should_we_talk_about/\
            I_wonder/Hard_to_write_in_an_uri_after_all/you_have_to_make\
            _up_the_punctuation_yourself/how_fun_is_that?test=foo&test1=\
            foo1&test2=foo2&test3=foo3&test4=foo4 HTTP/1.1\r\nHost: \
            hyper.rs\r\nAccept: a lot of things\r\nAccept-Charset: \
            utf8\r\nAccept-Encoding: *\r\nAccess-Control-Allow-\
            Credentials: None\r\nAccess-Control-Allow-Origin: None\r\n\
            Access-Control-Allow-Methods: None\r\nAccess-Control-Allow-\
            Headers: None\r\nContent-Encoding: utf8\r\nContent-Security-\
            Policy: None\r\nContent-Type: text/html\r\nOrigin: hyper\
            \r\nSec-Websocket-Extensions: It looks super important!\r\n\
            Sec-Websocket-Origin: hyper\r\nSec-Websocket-Version: 4.3\r\
            \nStrict-Transport-Security: None\r\nUser-Agent: hyper\r\n\
            X-Content-Duration: None\r\nX-Content-Security-Policy: None\
            \r\nX-DNSPrefetch-Control: None\r\nX-Frame-Options: \
            Something important obviously\r\nX-Requested-With: Nothing\
            \r\n\r\n".to_vec()
        );
        let len = raw.len();

        b.bytes = len as u64;
        b.iter(|| {
            ServerTransaction::parse(&mut raw).unwrap();
            restart(&mut raw, len);
        });


        fn restart(b: &mut BytesMut, len: usize) {
            b.reserve(1);
            unsafe {
                b.set_len(len);
            }
        }
    }

    /*
    #[cfg(feature = "nightly")]
    #[bench]
    fn bench_server_transaction_encode(b: &mut Bencher) {
        use header::{Headers, ContentLength, ContentType};
        use ::{StatusCode, HttpVersion};

        let len = 108;
        b.bytes = len as u64;

        let mut head = Parts {
            subject: StatusCode::Ok,
            headers: Headers::new(),
            version: HttpVersion::Http11,
        };
        head.headers.set(ContentLength(10));
        head.headers.set(ContentType::json());

        b.iter(|| {
            let mut vec = Vec::new();
            ServerTransaction::encode(head.clone(), true, &mut None, &mut vec);
            assert_eq!(vec.len(), len);
            ::test::black_box(vec);
        })
    }
    */
}
