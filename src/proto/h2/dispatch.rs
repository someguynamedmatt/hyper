use futures::{Stream};
use tokio_io::{AsyncRead, AsyncWrite};

use ::server::Service;

use super::server::Server;

//TODO: this whole module can go away now
pub fn server<T, S, B>(io: T, service: S) -> Server<T, S, B>
where
    T: AsyncRead + AsyncWrite,
    S: Service<Request = ::server::Request, Response = ::server::Response<B>, Error = ::Error>,
    B: Stream<Error=::Error>,
    B::Item: AsRef<[u8]> + 'static,
{
    Server::new(io, service)
}
