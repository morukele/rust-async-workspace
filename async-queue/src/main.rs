mod macros;

use std::pin::Pin;
use std::sync::LazyLock;
use std::task::{Context, Poll};
use std::time::Duration;
use std::{future::Future, panic::catch_unwind, thread};

use async_task::{Runnable, Task};
use flume::{Receiver, Sender};
use futures_lite::future;
use http::Uri;
use hyper::{Client, Request};

use anyhow::{Context as _, Error, Result, bail};
use smol::{Async, io, prelude::*};
use std::net::Shutdown;
use std::net::{TcpStream, ToSocketAddrs};

#[derive(Debug, Clone, Copy)]
enum FutureType {
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

fn spawn_task<F, T>(future: F, order: FutureType) -> Task<T>
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
            let mut results = Vec::new();
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

fn main() {
    Runtime::new().with_low_num(2).with_high_num(4).run();
    spawn_task!(BackgroundProcess {}).detach();

    let url = "http://www.rust-lang.org";
    let url: Uri = url.parse().unwrap();

    let request = Request::builder()
        .method("GET")
        .uri(url)
        .header("User-Agent", "hyper/0.14.2")
        .header("Accept", "text/html")
        .body(hyper::Body::empty())
        .unwrap();

    let future = async {
        let client = Client::new();
        client.request(request).await.unwrap()
    };
    let test = spawn_task!(future);
    let response = future::block_on(test);
    println!("Response status: {}", response.status());
}
