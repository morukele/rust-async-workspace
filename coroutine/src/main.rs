// File is full of error because it is using unstable features from nightly build.
// The features in question are related to coroutines.
// To run the file use `cargo +nightly run`

#![feature(coroutines)]
#![feature(coroutine_trait)]
use rand::{Rng, RngExt};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::Write;
use std::ops::{Coroutine, CoroutineState};
use std::pin::Pin;
use std::time::Instant;

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

fn append_number_to_file(n: i32) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("numbers.txt")?;
    writeln!(file, "{}", n)?;
    Ok(())
}

fn main() -> io::Result<()> {
    let mut rng = rand::rng();
    let numbers: Vec<i32> = (0..200_000).map(|_| rng.random()).collect();
    let mut coroutine = WriteCoroutine::new("numbers.txt")?;

    let start = Instant::now();

    for number in &numbers {
        Pin::new(&mut coroutine).resume(*number);
    }

    let duration = start.elapsed();
    println!(
        "The time elapsed for the write operation is: {:?}",
        duration
    );

    Ok(())
}
