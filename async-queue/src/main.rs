use std::pin::Pin;
use std::sync::LazyLock;
use std::task::{Context, Poll};
use std::time::Duration;
use std::{future::Future, panic::catch_unwind, thread};

use async_task::{Runnable, Task};
use flume::{Receiver, Sender};
use futures_lite::future;
use http::{Response, Uri};
use hyper::{Body, Client, Request};

use anyhow::{Context as _, Error, Result, bail};
use async_native_tls::TlsStream;
use hyper::client::connect::Connected;
use smol::{Async, prelude::*};
use std::net::Shutdown;
use std::net::{TcpStream, ToSocketAddrs};
use tokio::io::{ReadBuf, join};

#[derive(Debug, Clone, Copy)]
pub enum FutureType {
    High,
    Low,
}

struct CounterFuture {
    count: u32,
}

impl Future for CounterFuture {
    type Output = u32;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.count += 1;
        println!("polling with result: {}", self.count);
        thread::sleep(Duration::from_secs(1));
        if self.count < 3 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(self.count)
        }
    }
}

pub fn spawn_task<F, T>(future: F, order: FutureType) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    static HIGH_CHANNEL: LazyLock<(Sender<Runnable>, Receiver<Runnable>)> =
        LazyLock::new(flume::unbounded::<Runnable>);
    static LOW_CHANNEL: LazyLock<(Sender<Runnable>, Receiver<Runnable>)> =
        LazyLock::new(flume::unbounded::<Runnable>);

    static HIGH_QUEUE: LazyLock<Sender<Runnable>> = LazyLock::new(|| {
        let high_num = std::env::var("HIGH_NUM")
            .unwrap()
            .parse::<usize>()
            .unwrap_or(2);
        for _ in 0..high_num {
            // Stealing logic for high priority queue
            let high_receiver = HIGH_CHANNEL.1.clone();
            let low_receiver = LOW_CHANNEL.1.clone();
            thread::spawn(move || {
                loop {
                    match high_receiver.try_recv() {
                        Ok(runnable) => {
                            let _ = catch_unwind(|| runnable.run());
                        }
                        Err(_) => match low_receiver.try_recv() {
                            Ok(runnable) => {
                                let _ = catch_unwind(|| runnable.run());
                            }
                            Err(_) => {
                                thread::sleep(Duration::from_millis(100));
                            }
                        },
                    }
                }
            });
        }
        HIGH_CHANNEL.0.clone()
    });

    static LOW_QUEUE: LazyLock<Sender<Runnable>> = LazyLock::new(|| {
        let low_num = std::env::var("LOW_NUM")
            .unwrap()
            .parse::<usize>()
            .unwrap_or(1);
        for _ in 0..low_num {
            let low_receiver = LOW_CHANNEL.1.clone();
            let high_receiver = HIGH_CHANNEL.1.clone();
            thread::spawn(move || {
                // Stealing logic for low priority queue
                loop {
                    match low_receiver.try_recv() {
                        Ok(runnable) => {
                            let _ = catch_unwind(|| runnable.run());
                        }
                        Err(_) => match high_receiver.try_recv() {
                            Ok(runnable) => {
                                let _ = catch_unwind(|| runnable.run());
                            }
                            Err(_) => {
                                thread::sleep(Duration::from_millis(100));
                            }
                        },
                    }
                }
            });
        }
        LOW_CHANNEL.0.clone()
    });

    // NOTE: this is a closure that accepts a runnable, it is not executing anything
    let schedule_high = |runnable| HIGH_QUEUE.send(runnable).unwrap();
    let schedule_low = |runnable| LOW_QUEUE.send(runnable).unwrap();

    let schedule = match order {
        FutureType::High => schedule_high,
        FutureType::Low => schedule_low,
    };

    let (runnable, task) = async_task::spawn(future, schedule);
    runnable.schedule(); // NOTE: this is where the task is put in the queue using the schedule closure
    println!(
        "Here is the queue count - High Priority : {:?}; Low Priority: {:?}",
        HIGH_QUEUE.len(),
        LOW_QUEUE.len()
    );

    task
}

// Macro: helper function to spawn default task
macro_rules! spawn_task {
    ($future:expr) => {
        spawn_task!($future, FutureType::Low)
    };
    ($future:expr, $order:expr) => {
        spawn_task($future, $order)
    };
}

// Macro: helper join function, assumes no error
macro_rules! join {
    ($($future:expr),*) => {
        {
            let mut results = vec![];
            $(
                results.push(future::block_on($future));
            )*
            results
        }
    };
}

// Macro: helper join function, returns error if encountered
macro_rules! try_join {
    ($($future:expr),*) => {
        {
            let mut results = Vec::new();
            $(
                let result = catch_unwind(|| future::block_on($future));
                results.push(result);
            )*
            results
        }
    };
}

struct CustomExecutor;

impl<F: Future + Send + 'static> hyper::rt::Executor<F> for CustomExecutor {
    fn execute(&self, fut: F) {
        spawn_task!(async {
            println!("sending request");
            fut.await;
        })
        .detach();
    }
}

struct Runtime {
    high_num: usize,
    low_num: usize,
}

impl Runtime {
    pub fn new() -> Self {
        let num_cores = thread::available_parallelism().unwrap().get();
        Self {
            high_num: num_cores - 2,
            low_num: 1,
        }
    }

    pub fn with_high_num(mut self, num: usize) -> Self {
        self.high_num = num;
        self
    }

    pub fn with_low_num(mut self, num: usize) -> Self {
        self.low_num = num;
        self
    }

    pub fn run(&self) {
        unsafe {
            std::env::set_var("HIGH_NUM", self.high_num.to_string());
            std::env::set_var("LOW_NUM", self.low_num.to_string());
        };

        let high = spawn_task!(async {}, FutureType::High);
        let low = spawn_task!(async {}, FutureType::Low);

        join!(high, low);
    }
}

// A future that will never be polled
struct BackgroundProcess;

impl Future for BackgroundProcess {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        println!("background process firing");
        thread::sleep(Duration::from_secs(1));
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

// An enum for HTTP/S for the custom connector
enum CustomStream {
    Plain(Async<TcpStream>),
    Tls(TlsStream<Async<TcpStream>>),
}

#[derive(Clone)]
struct CustomConnector;

impl hyper::service::Service<Uri> for CustomConnector {
    type Response = CustomStream;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        Box::pin(async move {
            let host = req.host().context("cannot parse host")?;

            match req.scheme_str() {
                Some("http") => {
                    let socket_addr = {
                        let host = host.to_string();
                        let port = req.port_u16().unwrap_or(80);
                        smol::unblock(move || (host.as_str(), port).to_socket_addrs())
                            .await?
                            .next()
                            .context("cannot resolve address")?
                    };
                    let stream = Async::<TcpStream>::connect(socket_addr).await?;
                    Ok(CustomStream::Plain(stream))
                }
                Some("https") => {
                    let socket_addr = {
                        let host = host.to_string();
                        let port = req.port_u16().unwrap_or(443);
                        smol::unblock(move || (host.as_str(), port).to_socket_addrs())
                            .await?
                            .next()
                            .context("cannot resolve address")?
                    };
                    let stream = Async::<TcpStream>::connect(socket_addr).await?;
                    let stream = async_native_tls::connect(host, stream).await?;
                    Ok(CustomStream::Tls(stream))
                }
                scheme => bail!("unsupported scheme: {:?}", scheme),
            }
        })
    }
}

impl tokio::io::AsyncRead for CustomStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            CustomStream::Plain(s) => {
                Pin::new(s)
                    .poll_read(cx, buf.initialize_unfilled())
                    .map_ok(|size| {
                        buf.advance(size);
                    })
            }
            CustomStream::Tls(s) => {
                Pin::new(s)
                    .poll_read(cx, buf.initialize_unfilled())
                    .map_ok(|size| {
                        buf.advance(size);
                    })
            }
        }
    }
}

impl tokio::io::AsyncWrite for CustomStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            CustomStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            CustomStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            CustomStream::Plain(s) => Pin::new(s).poll_flush(cx),
            CustomStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            CustomStream::Plain(s) => {
                s.get_ref().shutdown(Shutdown::Write)?;
                Poll::Ready(Ok(()))
            }
            CustomStream::Tls(s) => Pin::new(s).poll_close(cx),
        }
    }
}

impl hyper::client::connect::Connection for CustomStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

async fn fetch(req: Request<Body>) -> Result<Response<Body>> {
    Ok(Client::builder()
        .executor(CustomExecutor)
        .build::<_, Body>(CustomConnector)
        .request(req)
        .await?)
}

fn main() {
    Runtime::new().with_low_num(2).with_high_num(4).run();
    let future = async {
        let req = Request::get("https://www.rust-lang.org")
            .body(Body::empty())
            .unwrap();

        let response = fetch(req).await.unwrap();
        let body_bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        println!("{}", html);
    };
    let test = spawn_task!(future);
    future::block_on(test);
}
