// File is full of error because it is using unstable features from nightly build.
// The features in question are related to coroutines.
// To run the file use `cargo +nightly run`

#![feature(coroutines)]
#![feature(coroutine_trait)]

use rand::{Rng, RngExt};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{BufRead, BufReader, Write};
use std::ops::{Coroutine, CoroutineState};
use std::pin::Pin;
use std::time::{Duration, Instant};

struct WriteCoroutine {
    pub file_handle: File,
}

impl WriteCoroutine {
    fn new(path: &str) -> io::Result<Self> {
        let file_handle = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file_handle })
    }
}

impl Coroutine<i32> for WriteCoroutine {
    type Yield = ();
    type Return = ();

    fn resume(mut self: Pin<&mut Self>, arg: i32) -> CoroutineState<Self::Yield, Self::Return> {
        writeln!(self.file_handle, "{}", arg).expect("failed to resume coroutine");
        CoroutineState::Yielded(())
    }
}

struct ReadCoroutine {
    lines: io::Lines<BufReader<File>>,
}

impl ReadCoroutine {
    fn new(path: &str) -> io::Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let lines = reader.lines();

        Ok(Self { lines })
    }
}

impl Coroutine<()> for ReadCoroutine {
    type Yield = i32;
    type Return = ();

    fn resume(mut self: Pin<&mut Self>, _arg: ()) -> CoroutineState<Self::Yield, Self::Return> {
        match self.lines.next() {
            Some(Ok(line)) => {
                if let Ok(number) = line.parse::<i32>() {
                    CoroutineState::Yielded(number)
                } else {
                    CoroutineState::Complete(())
                }
            }
            Some(Err(_)) => CoroutineState::Complete(()),
            None => CoroutineState::Complete(()),
        }
    }
}

struct CoroutineManager {
    reader: ReadCoroutine,
    writer: WriteCoroutine,
}

impl CoroutineManager {
    fn new(read_path: &str, write_path: &str) -> io::Result<Self> {
        let reader = ReadCoroutine::new(read_path)?;
        let writer = WriteCoroutine::new(write_path)?;

        Ok(Self { reader, writer })
    }

    fn run(&mut self) {
        let mut read_pin = Pin::new(&mut self.reader);
        let mut write_pin = Pin::new(&mut self.writer);

        while let CoroutineState::Yielded(n) = read_pin.as_mut().resume(()) {
            write_pin.as_mut().resume(n);
        }
    }
}

trait SymmetricCoroutine {
    type Input;
    type Output;
    fn resume_with_input(self: Pin<&mut Self>, input: Self::Input) -> Self::Output;
}

impl SymmetricCoroutine for ReadCoroutine {
    type Input = ();
    type Output = Option<i32>;

    fn resume_with_input(mut self: Pin<&mut Self>, input: Self::Input) -> Self::Output {
        if let Some(Ok(line)) = self.lines.next() {
            line.parse::<i32>().ok()
        } else {
            None
        }
    }
}

impl SymmetricCoroutine for WriteCoroutine {
    type Input = i32;
    type Output = ();

    fn resume_with_input(mut self: Pin<&mut Self>, input: Self::Input) -> Self::Output {
        writeln!(self.file_handle, "{}", input).expect("unable to write input into file");
    }
}

struct SleepCoroutine {
    pub start: Instant,
    pub duration: Duration,
}

impl SleepCoroutine {
    fn new(duration: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
        }
    }
}

impl Coroutine<()> for SleepCoroutine {
    type Yield = ();
    type Return = ();

    fn resume(self: Pin<&mut Self>, _arg: ()) -> CoroutineState<Self::Yield, Self::Return> {
        if self.start.elapsed() > self.duration {
            CoroutineState::Complete(())
        } else {
            CoroutineState::Yielded(())
        }
    }
}

type CustomCoroutine = Pin<Box<dyn Coroutine<(), Yield = (), Return = ()>>>;
struct Executor {
    coroutines: VecDeque<CustomCoroutine>,
}

impl Executor {
    fn new() -> Self {
        Self {
            coroutines: VecDeque::new(),
        }
    }

    fn add(&mut self, coroutine: CustomCoroutine) {
        self.coroutines.push_back(coroutine);
    }

    fn poll(&mut self) {
        println!("Polling {} coroutines", self.coroutines.len());
        let mut coroutine = self
            .coroutines
            .pop_front()
            .expect("no more coroutine in queue");

        match coroutine.as_mut().resume(()) {
            std::ops::CoroutineState::Yielded(_) => self.coroutines.push_back(coroutine),
            std::ops::CoroutineState::Complete(_) => {} // if it is completed, do nothing
        }
    }
}

fn main() -> io::Result<()> {
    let mut executor = Executor::new();
    for i in 0..3 {
        executor.add(Box::pin(SleepCoroutine::new(Duration::from_secs(1))));
    }

    let mut counter = 0;
    let start = Instant::now();

    while !executor.coroutines.is_empty() {
        executor.poll();
    }
    println!("Total time taken: {:?}", start.elapsed());

    Ok(())
}
