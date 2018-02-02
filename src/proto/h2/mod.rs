use bytes::Buf;
use futures::{Async, Future, Poll, Stream};
use h2::{Reason, SendStream};

use ::proto::h1::Cursor;

mod client;
mod server;

pub use self::client::Client;
pub use self::server::Server;

// body adpaters used by both Client and Server

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
            match self.stream.poll() {
                Ok(Async::Ready(Some(chunk))) => {
                    self.body_tx.send_data(SendBuf(Some(Cursor::new(chunk))), false)?;
                },
                Ok(Async::Ready(None)) => {
                    self.body_tx.send_data(SendBuf(None), true)?;
                    return Ok(Async::Ready(()));
                },
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(err) => {
                    self.body_tx.send_reset(Reason::INTERNAL_ERROR);
                    return Err(err);
                }
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
