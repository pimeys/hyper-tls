use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::client::connect::{Connected, Destination, HttpConnector};
pub use native_tls::Error;
use tokio::net::TcpStream;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_tls::TlsConnector;
//use tokio_net::tcp::TcpStream;
use tower_service::Service;

use crate::stream::MaybeHttpsStream;

/// A Connector for the `https` scheme.
#[derive(Clone)]
pub struct HttpsConnector {
    force_https: bool,
    http: HttpConnector,
    tls: TlsConnector,
}

impl HttpsConnector {
    /// Construct a new HttpsConnector.
    ///
    /// Takes number of DNS worker threads.
    ///
    /// This uses hyper's default `HttpConnector`, and default `TlsConnector`.
    /// If you wish to use something besides the defaults, use `From::from`.
    ///
    /// # Note
    ///
    /// By default this connector will use plain HTTP if the URL provded uses
    /// the HTTP scheme (eg: http://example.com/).
    ///
    /// If you would like to force the use of HTTPS then call https_only(true)
    /// on the returned connector.
    pub fn new() -> Result<Self, Error> {
        native_tls::TlsConnector::new().map(|tls| HttpsConnector::new_(tls.into()))
    }

    fn new_(tls: TlsConnector) -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        HttpsConnector::from((http, tls))
    }
}

impl From<(HttpConnector, TlsConnector)> for HttpsConnector {
    fn from(args: (HttpConnector, TlsConnector)) -> HttpsConnector {
        HttpsConnector {
            force_https: false,
            http: args.0,
            tls: args.1,
        }
    }
}

impl fmt::Debug for HttpsConnector {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("HttpsConnector")
            .field("force_https", &self.force_https)
            .field("http", &self.http)
            .finish()
    }
}

impl Service<Destination> for HttpsConnector {
    type Response = (MaybeHttpsStream<TcpStream>, Connected);
    type Error = io::Error;
    type Future = HttpsConnecting<TcpStream>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // For now, always ready.
        // TODO: When `Resolve` becomes an alias for `Service`, check
        // the resolver's readiness.
        drop(cx);
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Destination) -> Self::Future {
        let is_https = dst.scheme() == "https";
        // Early abort if HTTPS is forced but can't be used
        if !is_https && self.force_https {
            let err = io::Error::new(
                io::ErrorKind::Other,
                "HTTPS scheme forced but can't be used",
            );
            return HttpsConnecting(Box::pin(async { Err(err) }));
        }

        let host = dst.host().to_owned();
        let connecting = self.http.call(dst);
        let tls = self.tls.clone();

        let fut = async move {
            let (tcp, connected) = connecting.await.map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("HTTP Connection failed: {:?}", e),
                )
            })?;

            let maybe = if is_https {
                let tls = tls
                    .connect(&host, tcp)
                    .await
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                MaybeHttpsStream::Https(tls)
            } else {
                MaybeHttpsStream::Http(tcp)
            };

            Ok((maybe, connected))
        };

        HttpsConnecting(Box::pin(fut))
    }
}

type BoxedFut<T> =
    Pin<Box<dyn Future<Output = io::Result<(MaybeHttpsStream<T>, Connected)>> + Send>>;

/// A Future representing work to connect to a URL, and a TLS handshake.
pub struct HttpsConnecting<T>(BoxedFut<T>);

impl<T: AsyncRead + AsyncWrite + Unpin> Future for HttpsConnecting<T> {
    type Output = Result<(MaybeHttpsStream<T>, Connected), io::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> fmt::Debug for HttpsConnecting<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("HttpsConnecting")
    }
}
