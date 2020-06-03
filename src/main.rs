use h2::{server, RecvStream, SendStream};
use http::{Response, StatusCode};
use tokio::io::{AsyncRead,AsyncWrite};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use futures::{StreamExt,future::FutureExt};
use futures::io::{Error, ErrorKind};
use bytes::buf::ext::Reader;
use bytes::{buf::BufExt, Bytes, BytesMut};
use std::io::{Read, Write};
use bytes::buf::BufMutExt;

struct AsyncWritePart{
    send_stream:SendStream<Bytes>,
}

impl AsyncWritePart{
    fn new(send_stream:SendStream<Bytes>)->Self{
        Self{
            send_stream,
        }
    }
}

impl AsyncWrite for AsyncWritePart{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, Error>> {
        let p_self = self.get_mut();
        let send_stream = &mut p_self.send_stream;
        send_stream.reserve_capacity(buf.len());
        let poll= send_stream.poll_capacity(cx);
        match poll {
            Poll::Ready(Some(ret)) => match ret {
                Ok(size) => {
                    let cap = send_stream.capacity();
                    let mut send_buf = BytesMut::with_capacity(size).writer();
                    let write_num = send_buf.write(buf).unwrap();
                    let ret = send_stream.send_data(send_buf.into_inner().freeze(),false);
                    if let Err(e) = ret{
                        return Poll::Ready(Err(Error::new(ErrorKind::Other, format!("oh no! {}",e))));
                    }
                    return Poll::Ready(Ok(write_num))
                }
                Err(e) => return Poll::Ready(Err(Error::new(ErrorKind::Other, format!("oh no! {}",e)))),
            },
            Poll::Ready(None) => {
                return Poll::Ready(Err(Error::new(ErrorKind::Other, "oh no!")));
            }
            Poll::Pending => {
                return Poll::Pending;
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        return Poll::Ready(Ok(()));
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        return Poll::Ready(Ok(()));
    }
}

struct AsyncReadPart {
    inner: RecvStream,
    buf: Reader<Bytes>,
}

impl AsyncReadPart{
    fn new(inner:RecvStream)->Self{
        Self{
            inner,
            buf: Bytes::new().reader()
        }
    }
}

impl AsyncRead for AsyncReadPart{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        let p_self = self.get_mut();
        loop {
            let size = p_self.buf.read(buf)?;
            if size > 0 {
                return Poll::Ready(Ok(size));
            }
            let poll = p_self.inner.poll_next_unpin(cx);
            match poll {
                Poll::Ready(Some(ret)) => match ret {
                    Ok(chunk) => {
                        p_self.inner.flow_control().release_capacity(chunk.len()).unwrap();
                        let cap = p_self.inner.flow_control().available_capacity();
                        p_self.buf = chunk.reader();
                    }
                    Err(e) => return Poll::Ready(Err(Error::new(ErrorKind::Other, format!("oh no! {}",e)))),
                },
                Poll::Ready(None) => {
                    return Poll::Ready(Ok(0));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

pub fn run_client(){
    let client_task = async{

        let tcp = tokio::net::TcpStream::connect("127.0.0.1:5928").await.unwrap();
        tcp.set_nodelay(true);
        let (mut client, h2) = h2::client::handshake(tcp).await.unwrap();
        tokio::spawn(async move {
            if let Err(e) = h2.await {
                println!("GOT ERR={:?}", e);
            }
        });

        let mut listener = tokio::net::TcpListener::bind("127.0.0.1:2022").await.unwrap();
        while let Some(stream) = listener.next().await {
            match stream {
                Ok(mut stream) => {
                    let request = http::Request::builder()
                        .uri("https://127.0.0.1/")
                        .body(())
                        .unwrap();
                    let (response, send_stream) = client.send_request(request, false).unwrap();
                    let response = response.await.unwrap();
                    let recv_stream:RecvStream = response.into_body();

                    let stream_session = async move{
                        let (mut tcp_read, mut tcp_write) = stream.split();
                        let mut h2_read = AsyncReadPart::new(recv_stream);
                        let mut h2_write = AsyncWritePart::new(send_stream);
                        let cp1 = tokio::io::copy(&mut h2_read,&mut tcp_write).then(|_|{
                            futures::future::err::<(),()>(())
                        });
                        let cp2 = tokio::io::copy(&mut tcp_read,&mut h2_write).then(|_|{
                            futures::future::err::<(),()>(())
                        });
                        let _ = tokio::try_join!(cp1, cp2);
                        println!("client session closed");
                    };
                    tokio::spawn(stream_session);
                }
                Err(e) => {
                    println!("listen failed {}",e);
                }
            }
        }
    };
    //let mut rt = tokio::runtime::Runtime::new().unwrap();
    let mut basic_rt = tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()
        .unwrap();
    basic_rt.block_on(client_task);
}

pub fn run_server(){
    let server_task = async {
        let mut listener = tokio::net::TcpListener::bind("127.0.0.1:5928").await.unwrap();
        loop {
            if let Ok((socket, _peer_addr)) = listener.accept().await {
                tokio::spawn(async {
                    socket.set_nodelay(true);
                    let mut h2 = server::handshake(socket).await.unwrap();
                    while let Some(request) = h2.accept().await {
                        let stream_session = async move{
                            let (request, mut respond) = request.unwrap();
                            let recv_stream = request.into_body();
                            let response = Response::builder()
                                .status(StatusCode::OK)
                                .body(())
                                .unwrap();
                            let send_stream = respond.send_response(response, false)
                                .unwrap();

                            let mut tcp_stream = tokio::net::TcpStream::connect("127.0.0.1:22").await.unwrap();

                            let (mut tcp_read, mut tcp_write) = tcp_stream.split();

                            let mut h2_read = AsyncReadPart::new(recv_stream);
                            let mut h2_write = AsyncWritePart::new(send_stream);

                            let cp1 = tokio::io::copy(&mut h2_read,&mut tcp_write).then(|_|{
                                futures::future::err::<(),()>(())
                            });
                            let cp2 = tokio::io::copy(&mut tcp_read,&mut h2_write).then(|_|{
                                futures::future::err::<(),()>(())
                            });
                            let _ = tokio::try_join!(cp1, cp2);

                            println!("server session closed");
                        };
                        tokio::spawn(stream_session);
                    }
                });
            }
        }
    };
    let mut basic_rt = tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()
        .unwrap();
    //let mut rt = tokio::runtime::Runtime::new().unwrap();
    basic_rt.block_on(server_task);
}

fn main() {
    if std::env::args().nth(1) == Some("server".to_string()) {
        println!("Starting server ......");
        run_server();
    } else {
        println!("Starting client ......");
        run_client();
    }
}
