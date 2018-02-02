//#![deny(warnings)]
extern crate futures;
extern crate hyper;
extern crate tokio_core;

extern crate pretty_env_logger;

use std::env;
use std::io::{self, Write};

use futures::Future;
use futures::stream::Stream;

use hyper::{Client, HttpVersion, Request};

fn main() {
    pretty_env_logger::init();

    let url = match env::args().nth(1) {
        Some(url) => url,
        None => {
            println!("Usage: client <url>");
            return;
        }
    };

    let url = url.parse::<hyper::Uri>().unwrap();
    if url.scheme() != Some("http") {
        println!("This example only works with 'http' URLs.");
        return;
    }

    let mut core = tokio_core::reactor::Core::new().unwrap();
    let handle = core.handle();
    let client = Client::new(&handle);

    let mut req = Request::new(::hyper::Get, url.clone());
    req.set_version(HttpVersion::Http2);

    let work1 = client.request(req).and_then(|res| {
        println!("Version: {}", res.version());
        println!("Status: {}", res.status());
        println!("Headers: \n{}", res.headers());

        res.body().for_each(|chunk| {
            io::stdout().write_all(&chunk).map_err(From::from)
        })
    }).map(|_| {
        println!("\n\nDone 1.");
    });

    let mut req = Request::new(::hyper::Get, url);
    req.set_version(HttpVersion::Http2);

    let work2 = client.request(req).and_then(|res| {
        println!("Version: {}", res.version());
        println!("Status: {}", res.status());
        println!("Headers: \n{}", res.headers());

        res.body().for_each(|chunk| {
            io::stdout().write_all(&chunk).map_err(From::from)
        })
    }).map(|_| {
        println!("\n\nDone 2.");
    });

    core.run(work1.join(work2)).unwrap();
}
