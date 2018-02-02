use bytes::Buf;
use futures::{Async, Future, Poll, Stream};
use h2::SendStream;

use ::proto::h1::Cursor;

mod client;
mod dispatch;
mod server;

pub(crate) use self::dispatch::{server};
pub use self::client::Client;
pub use self::server::Server;

struct PipeToSendStream<S>
where
    S: Stream,
    S::Item: AsRef<[u8]> + 'static,
{
    body_tx: SendStream<SendBuf<S::Item>>,
    stream: S,
}

impl<S> PipeToSendStream<S>
where
    S: Stream<Error=::Error>,
    S::Item: AsRef<[u8]> + 'static,
{
    fn new(stream: S, tx: SendStream<SendBuf<S::Item>>) -> PipeToSendStream<S> {
        PipeToSendStream {
            body_tx: tx,
            stream: stream,
        }
    }
}

impl<S> Future for PipeToSendStream<S>
where
    S: Stream<Error=::Error>,
    S::Item: AsRef<[u8]> + 'static,
{
    type Item = ();
    type Error = ::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            //TODO: make use of flow control on SendStream
            if let Some(chunk) = try_ready!(self.stream.poll()) {
                self.body_tx.send_data(SendBuf(Some(Cursor::new(chunk))), false)?;
            } else {
                self.body_tx.send_data(SendBuf(None), true)?;
                return Ok(Async::Ready(()));
            }
        }
    }
}

struct SendBuf<B>(Option<Cursor<B>>);

impl<B: AsRef<[u8]>> Buf for SendBuf<B> {
    #[inline]
    fn remaining(&self) -> usize {
        self.0
            .as_ref()
            .map(|b| b.remaining())
            .unwrap_or(0)
    }

    #[inline]
    fn bytes(&self) -> &[u8] {
        self.0
            .as_ref()
            .map(|b| b.bytes())
            .unwrap_or(&[])
    }

    #[inline]
    fn advance(&mut self, cnt: usize) {
        self.0
            .as_mut()
            .map(|b| b.advance(cnt));
    }
}
