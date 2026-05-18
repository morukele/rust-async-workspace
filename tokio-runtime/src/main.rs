use std::cell::{RefCell, UnsafeCell};
use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::runtime::{Builder, Runtime};
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinHandle;
use tokio_util::task::LocalPoolHandle;

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(1)
        .on_thread_start(|| {
            println!("thread starting for runtime A");
        })
        .on_thread_stop(|| {
            println!("thread stopping for runtime A");
        })
        .thread_keep_alive(Duration::from_secs(60))
        .global_queue_interval(61)
        .on_thread_park(|| {
            println!("thread parking for runtime A");
        })
        .thread_name("our custom runtime A")
        .thread_stack_size(3 * 1024 * 1024)
        .enable_time()
        .build()
        .unwrap()
});

static HIGH_PRIORITY: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("High Priorty Runtime")
        .enable_time()
        .build()
        .unwrap()
});

static LOW_PRIORITY: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("Low Priority Runtime")
        .enable_time()
        .build()
        .unwrap()
});

pub fn spawn_task<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    RUNTIME.spawn(future)
}

thread_local! {
    pub static COUNTER: UnsafeCell<HashMap<u32, u32>> = UnsafeCell::new(HashMap::new());
}

async fn something(number: u32) {
    tokio::time::sleep(Duration::from_secs(number as u64)).await;
    COUNTER.with(|counter| {
        let counter = unsafe { &mut *counter.get() };
        match counter.get_mut(&number) {
            Some(count) => {
                let placeholder = *count + 1;
                *count = placeholder;
            }
            None => {
                counter.insert(number, 1);
            }
        }
    });
}

async fn print_statement() {
    COUNTER.with(|counter| {
        let counter = unsafe { &mut *counter.get() };
        println!("Counter: {:?}", counter);
    });
}

static POOL_RUNTIME: LazyLock<LocalPoolHandle> = LazyLock::new(|| LocalPoolHandle::new(4));

fn extract_data_from_thread() -> HashMap<u32, u32> {
    let mut extracted_counter: HashMap<u32, u32> = HashMap::new();
    COUNTER.with(|counter| {
        let counter = unsafe { &mut *counter.get() };
        extracted_counter = counter.clone();
    });

    extracted_counter
}

async fn get_complete_count() -> HashMap<u32, u32> {
    let mut complete_counter = HashMap::new();
    let mut extracted_counters = Vec::new();
    for i in 0..4 {
        extracted_counters.push(
            POOL_RUNTIME.spawn_pinned_by_idx(|| async move { extract_data_from_thread() }, i),
        );
    }
    for counter_future in extracted_counters {
        let extracted_counter = counter_future.await.unwrap_or_default();
        for (key, count) in extracted_counter {
            *complete_counter.entry(key).or_insert(0) += count;
        }
    }

    complete_counter
}

#[tokio::main]
async fn main() {
    tokio::spawn(async {
        let sequence = [1, 2, 3, 4, 5];
        let repeated_sequence: Vec<_> = sequence.iter().cycle().take(500_000).cloned().collect();

        let mut futures = Vec::new();
        for number in repeated_sequence {
            futures.push(POOL_RUNTIME.spawn_pinned(move || async move {
                something(number).await;
                something(number).await;
            }));
        }

        for i in futures {
            i.await.unwrap();
        }
        println!("All futures completed");
    });
    let pid = std::process::id();
    println!("The PID of this process is: {}", pid);
    let mut stream = signal(SignalKind::hangup()).unwrap();
    stream.recv().await;
    let completed_counter = get_complete_count().await;
    println!("Complete counter: {:?}", completed_counter);
}
