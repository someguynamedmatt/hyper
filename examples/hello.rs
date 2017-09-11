#![deny(warnings)]
extern crate http;
extern crate hyper;
extern crate futures;
extern crate pretty_env_logger;

use futures::future::FutureResult;

use http::{Request, Response};
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};

use hyper::Body;
use hyper::server::{Http, Service};

static BODY: &'static str = "Hello World!";
static LEN: &'static str = "12";
static MIME: &'static str = "text/plain";

struct Hello;

impl Service for Hello {
    type Request = Request<Option<Body>>;
    type Response = Response<Option<Body>>;
    type Error = hyper::Error;
    type Future = FutureResult<Self::Response, hyper::Error>;
    fn call(&self, _req: Self::Request) -> Self::Future {
        let mut resp = Response::new(Some(Body::from(BODY)));
        resp.headers_mut().insert(CONTENT_LENGTH, HeaderValue::from_static(LEN));
        resp.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(MIME));
        futures::future::ok(resp)
    }

}

fn main() {
    pretty_env_logger::init().unwrap();
    let addr = "127.0.0.1:3000".parse().unwrap();
    let server = Http::new().bind(&addr, || Ok(Hello)).unwrap();
    println!("Listening on http://{} with 1 thread.", server.local_addr().unwrap());
    server.run().unwrap();
}
