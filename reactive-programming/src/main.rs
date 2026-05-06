use core::sync::atomic::Ordering;
use device_query::{DeviceEvents, DeviceState};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU32};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

static TEMP: LazyLock<Arc<AtomicI16>> = LazyLock::new(|| Arc::new(AtomicI16::new(2090)));
static DESIRED_TEMP: LazyLock<Arc<AtomicI16>> = LazyLock::new(|| Arc::new(AtomicI16::new(2100)));
static HEAT_ON: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

pub struct DisplayFuture {
    pub temp_snapshot: i16,
}

impl Default for DisplayFuture {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayFuture {
    pub fn new() -> Self {
        DisplayFuture {
            temp_snapshot: TEMP.load(Ordering::SeqCst),
        }
    }
}

impl Future for DisplayFuture {
    type Output = ();

    // This future never pools because we want to continually
    // check the temperature until the application is shut down
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current_snapshot = TEMP.load(Ordering::SeqCst);
        let desired_temp = DESIRED_TEMP.load(Ordering::SeqCst);
        let heat_on = HEAT_ON.load(Ordering::SeqCst);

        // if no change, quick return by ending the poll
        if current_snapshot == self.temp_snapshot {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        if current_snapshot < desired_temp && !heat_on {
            // turn heat on
            HEAT_ON.store(true, Ordering::SeqCst);
        } else if current_snapshot > desired_temp && heat_on {
            // turn heat off
            HEAT_ON.store(false, Ordering::SeqCst);
        }

        clearscreen::clear().unwrap();

        println!(
            "Temperature: {}\nDesired Temp: {}\nHeater On: {}",
            current_snapshot as f32 / 100.0,
            desired_temp as f32 / 100.0,
            heat_on
        );
        self.temp_snapshot = current_snapshot;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

pub struct HeaterFuture {
    pub time_snapshot: Instant,
}

impl Default for HeaterFuture {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaterFuture {
    pub fn new() -> Self {
        HeaterFuture {
            time_snapshot: Instant::now(),
        }
    }
}

impl Future for HeaterFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !HEAT_ON.load(Ordering::SeqCst) {
            self.time_snapshot = Instant::now();
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let current_snapshot = Instant::now();
        if current_snapshot.duration_since(self.time_snapshot) < Duration::from_secs(3) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        TEMP.fetch_add(3, Ordering::SeqCst);
        self.time_snapshot = Instant::now();
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

pub struct HeatLossFuture {
    pub time_snapshot: Instant,
}

impl Default for HeatLossFuture {
    fn default() -> Self {
        Self::new()
    }
}

impl HeatLossFuture {
    pub fn new() -> Self {
        HeatLossFuture {
            time_snapshot: Instant::now(),
        }
    }
}

impl Future for HeatLossFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current_snapshot = Instant::now();
        if current_snapshot.duration_since(self.time_snapshot) > Duration::from_secs(3) {
            TEMP.fetch_sub(1, Ordering::SeqCst);
            self.time_snapshot = Instant::now();
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

fn perform_operation_with_callback<F>(callback: F)
where
    F: Fn(i32),
{
    let result = 42;
    callback(result);
}

static INPUT: LazyLock<Arc<Mutex<String>>> = LazyLock::new(|| Arc::new(Mutex::new(String::new())));
static DEVICE_STATE: LazyLock<Arc<DeviceState>> = LazyLock::new(|| Arc::new(DeviceState::new()));

pub fn render(temp: i16, desired_temp: i16, heat_on: bool, input: String) {
    clearscreen::clear().unwrap();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    println!(
        "Temperature: {}\nDesired Temp: {}\nHeater On: {}",
        temp as f32 / 100.0,
        desired_temp as f32 / 100.0,
        heat_on
    );
    print!("Input: {}", input);
    handle.flush().unwrap();
}

pub struct EventHandle<'a, T: Clone + Send> {
    pub id: u32,
    event_bus: Arc<&'a EventBus<T>>,
}

impl<'a, T: Clone + Send> EventHandle<'a, T> {
    pub async fn poll(&self) -> Option<T> {
        self.event_bus.poll(self.id).await
    }
}

impl<'a, T: Clone + Send> Drop for EventHandle<'a, T> {
    fn drop(&mut self) {
        self.event_bus.unsubscribe(self.id);
    }
}

// Events are denoted by T, they can be cloned and sent down threads/channels
pub struct EventBus<T: Clone + Send> {
    chamber: AsyncMutex<HashMap<u32, VecDeque<T>>>,
    count: AtomicU32,
    dead_ids: Mutex<Vec<u32>>,
}

impl<T: Clone + Send> Default for EventBus<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Send> EventBus<T> {
    pub fn new() -> Self {
        Self {
            chamber: AsyncMutex::new(HashMap::new()),
            count: AtomicU32::new(0),
            dead_ids: Mutex::new(Vec::new()),
        }
    }

    pub async fn subscribe(&self) -> EventHandle<T> {
        let mut chamber = self.chamber.lock().await;
        let id = self.count.fetch_add(1, Ordering::SeqCst);
        chamber.insert(id, VecDeque::new());
        EventHandle {
            id,
            event_bus: Arc::new(self),
        }
    }

    pub fn unsubscribe(&self, id: u32) {
        self.dead_ids.lock().unwrap().push(id);
    }

    pub async fn poll(&self, id: u32) -> Option<T> {
        let mut chamber = self.chamber.lock().await;
        let queue = chamber.get_mut(&id).unwrap();
        queue.pop_front()
    }

    pub async fn send(&self, event: T) {
        let mut chamber = self.chamber.lock().await;
        for (_, value) in chamber.iter_mut() {
            value.push_back(event.clone());
        }
    }
}

async fn consume_event_bus(event_bus: Arc<EventBus<f32>>) {
    let handle = event_bus.subscribe().await;
    loop {
        let event = handle.poll().await;
        if let Some(event) = event {
            println!("id: {} value: {}", handle.id, event);
            if event == 3.0 {
                break;
            }
        }
    }
}

async fn garbage_collector(event_bus: Arc<EventBus<f32>>) {
    loop {
        let mut chamber = event_bus.chamber.lock().await;
        let dead_ids = event_bus.dead_ids.lock().unwrap().clone();
        event_bus.dead_ids.lock().unwrap().clear();
        for id in dead_ids {
            chamber.remove(&id);
        }
        std::mem::drop(chamber);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[tokio::main]
async fn main() {
    let event_bus = Arc::new(EventBus::<f32>::new());
    let bus_one = event_bus.clone();
    let bus_two = event_bus.clone();
    let gb_bus_ref = event_bus.clone();

    let _gb = tokio::task::spawn(async { garbage_collector(gb_bus_ref).await });
    let one = tokio::task::spawn(async { consume_event_bus(bus_one).await });
    let two = tokio::task::spawn(async { consume_event_bus(bus_two).await });

    std::thread::sleep(Duration::from_secs(1)); // blocking on purpose
    event_bus.send(1.0).await;
    event_bus.send(2.0).await;
    event_bus.send(3.0).await;

    let _ = one.await;
    let _ = two.await;
    println!("{:?}", event_bus.chamber.lock().await);
    std::thread::sleep(Duration::from_secs(3));
    println!("{:?}", event_bus.chamber.lock().await);
}
