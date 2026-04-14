use mio::net::TcpListener;
use mio::{Events, Poll as MioPoll, Token};
use std::error::Error;
use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

// TODO: convert the runtime into a library to use for the project

const SERVER: Token = Token(0);
const CLIENT: Token = Token(1);

struct ServerFuture {
    server: TcpListener,
    poll: MioPoll,
}

impl Future for ServerFuture {
    type Output = String;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut events = Events::with_capacity(1);
        self.poll
            .poll(&mut events, Some(Duration::from_millis(2000)))
            .unwrap();

        for event in events.iter() {
            if event.token() == SERVER && event.is_readable() {
                let (mut stream, _) = self.server.accept().unwrap();
                let mut buffer = [0u8; 1024];
                let mut received_data = Vec::new();

                loop {
                    match stream.read(&mut buffer) {
                        Ok(n) if n > 0 => {
                            received_data.extend_from_slice(&buffer[..n]);
                        }
                        Ok(_) => break,
                        Err(e) => {
                            eprintln!("Error reading the stream: {}", e);
                            break;
                        }
                    }
                }

                if !received_data.is_empty() {
                    let received_str = String::from_utf8_lossy(&received_data);
                    return Poll::Ready(received_str.to_string());
                }

                // return pending because received_data is empty
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    Ok(())
}
