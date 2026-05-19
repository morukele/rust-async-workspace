use std::collections::HashMap;
use std::io;
use std::sync::OnceLock;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{
    mpsc::channel,
    mpsc::{Receiver, Sender},
    oneshot,
};
use tokio::time::{self, Duration, Instant};

struct SetKeyValueMessage {
    key: String,
    value: Vec<u8>,
    response: oneshot::Sender<()>,
}

struct GetKeyValueMessage {
    key: String,
    response: oneshot::Sender<Option<Vec<u8>>>,
}

struct DeleteKeyValueMessage {
    key: String,
    response: oneshot::Sender<()>,
}

enum KeyValueMessage {
    Get(GetKeyValueMessage),
    Delete(DeleteKeyValueMessage),
    Set(SetKeyValueMessage),
}

enum RoutingMessage {
    KeyValue(KeyValueMessage),
    Heartbeat(ActorType),
    Reset(ActorType),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ActorType {
    KeyValue,
    Writer,
}

async fn key_value_actor(mut receiver: Receiver<KeyValueMessage>) {
    let (writer_key_value_sender, writer_key_value_receiver) = channel(32);
    let _writer_handle = tokio::spawn(writer_actor(writer_key_value_receiver));

    // Getting the state of the map from the writer actor
    let (get_sender, get_receiver) = oneshot::channel();
    let _ = writer_key_value_sender
        .send(WriterLogMessage::Get(get_sender))
        .await; // consider unwrapping later
    let mut map = get_receiver.await.unwrap();

    let timeout_duration = Duration::from_millis(200);
    let router_sender = ROUTER_SENDER.get().unwrap().clone();

    loop {
        match time::timeout(timeout_duration, receiver.recv()).await {
            Ok(Some(message)) => {
                while let Some(message) = receiver.recv().await {
                    if let Some(write_message) = WriterLogMessage::from_key_value_message(&message)
                    {
                        let _ = writer_key_value_sender.send(write_message).await;
                    }
                    match message {
                        KeyValueMessage::Get(data) => {
                            let _ = data.response.send(map.get(&data.key).cloned());
                        }
                        KeyValueMessage::Delete(data) => {
                            map.remove(&data.key);
                            let _ = data.response.send(());
                        }
                        KeyValueMessage::Set(data) => {
                            map.insert(data.key, data.value);
                            let _ = data.response.send(());
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {
                router_sender
                    .send(RoutingMessage::Heartbeat(ActorType::KeyValue))
                    .await
                    .unwrap();
            }
        }
    }
}

async fn router(mut receiver: Receiver<RoutingMessage>) {
    let (mut key_value_sender, mut key_value_receiver) = channel(32);
    let mut key_value_handle = tokio::spawn(key_value_actor(key_value_receiver));

    let (heartbeat_sender, heartbeat_receiver) = channel(32);
    tokio::spawn(heartbeat_actor(heartbeat_receiver));

    while let Some(message) = receiver.recv().await {
        match message {
            RoutingMessage::KeyValue(message) => {
                let _ = key_value_sender.send(message).await;
            }
            RoutingMessage::Heartbeat(message) => {
                let _ = heartbeat_sender.send(message).await;
            }
            RoutingMessage::Reset(message) => match message {
                ActorType::KeyValue | ActorType::Writer => {
                    let (new_key_value_sender, new_key_value_receiver) = channel(32);
                    key_value_handle.abort();
                    key_value_sender = new_key_value_sender;
                    key_value_receiver = new_key_value_receiver;
                    key_value_handle = tokio::spawn(key_value_actor(key_value_receiver));
                    time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
}

static ROUTER_SENDER: OnceLock<Sender<RoutingMessage>> = OnceLock::new();

async fn set(key: String, value: Vec<u8>) -> Result<(), std::io::Error> {
    let (tx, rx) = oneshot::channel();
    ROUTER_SENDER
        .get()
        .unwrap()
        .send(RoutingMessage::KeyValue(KeyValueMessage::Set(
            SetKeyValueMessage {
                key,
                value,
                response: tx,
            },
        )))
        .await
        .unwrap();
    rx.await.unwrap();
    Ok(())
}

async fn get(key: String) -> Result<Option<Vec<u8>>, std::io::Error> {
    let (tx, rx) = oneshot::channel();
    ROUTER_SENDER
        .get()
        .unwrap()
        .send(RoutingMessage::KeyValue(KeyValueMessage::Get(
            GetKeyValueMessage { key, response: tx },
        )))
        .await
        .unwrap();

    Ok(rx.await.unwrap())
}

async fn delete(key: String) -> Result<(), std::io::Error> {
    let (tx, rx) = oneshot::channel();
    ROUTER_SENDER
        .get()
        .unwrap()
        .send(RoutingMessage::KeyValue(KeyValueMessage::Delete(
            DeleteKeyValueMessage { key, response: tx },
        )))
        .await
        .unwrap();
    rx.await.unwrap();
    Ok(())
}

enum WriterLogMessage {
    Set(String, Vec<u8>),
    Delete(String),
    Get(oneshot::Sender<HashMap<String, Vec<u8>>>),
}

impl WriterLogMessage {
    fn from_key_value_message(message: &KeyValueMessage) -> Option<WriterLogMessage> {
        match message {
            KeyValueMessage::Get(_) => None,
            KeyValueMessage::Delete(message) => Some(WriterLogMessage::Delete(message.key.clone())),
            KeyValueMessage::Set(message) => Some(WriterLogMessage::Set(
                message.key.clone(),
                message.value.clone(),
            )),
        }
    }
}

async fn read_data_from_file(file_path: &str) -> io::Result<HashMap<String, Vec<u8>>> {
    let mut file = File::open(file_path).await?;
    let mut contents = String::new(); // this acts as a buffer for which the file data is read into
    file.read_to_string(&mut contents).await?;
    let data: HashMap<String, Vec<u8>> = serde_json::from_str(&contents)?;
    Ok(data)
}

async fn load_map(file_path: &str) -> HashMap<String, Vec<u8>> {
    match read_data_from_file(file_path).await {
        Ok(data) => {
            println!("Data loaded from file: {:?}", data);
            data
        }
        Err(err) => {
            println!("Failed to read from file: {:?}", err);
            println!("Starting with an empty hashmap.");
            HashMap::new()
        }
    }
}

async fn writer_actor(mut receiver: Receiver<WriterLogMessage>) -> io::Result<()> {
    let mut map = load_map("./data.json").await;
    let mut file = File::create("./data.json").await.unwrap();

    let timeout_duration = Duration::from_millis(200);
    let router_sender = ROUTER_SENDER.get().unwrap().clone();

    loop {
        match time::timeout(timeout_duration, receiver.recv()).await {
            Ok(Some(message)) => {
                match message {
                    WriterLogMessage::Set(key, value) => {
                        map.insert(key, value);
                    }
                    WriterLogMessage::Delete(key) => {
                        map.remove(&key);
                    }
                    WriterLogMessage::Get(response) => {
                        let _ = response.send(map.clone());
                    }
                }
                let contents = serde_json::to_string(&map).unwrap();
                file.set_len(0).await?;
                file.seek(std::io::SeekFrom::Start(0)).await?;
                file.write_all(contents.as_bytes()).await?;
                file.flush().await?;
            }
            Ok(None) => break,
            Err(_) => router_sender
                .send(RoutingMessage::Heartbeat(ActorType::Writer))
                .await
                .unwrap(),
        }
    }
    Ok(())
}

async fn heartbeat_actor(mut receiver: Receiver<ActorType>) {
    let mut map = HashMap::new();
    let timeout_duration = Duration::from_millis(200);
    loop {
        match time::timeout(timeout_duration, receiver.recv()).await {
            Ok(Some(actor_name)) => map.insert(actor_name, Instant::now()),
            Ok(None) => break,
            Err(_) => {
                continue;
            }
        };
        let half_second_ago = Instant::now() - Duration::from_millis(500);
        for (key, &value) in map.iter() {
            if value < half_second_ago {
                match key {
                    ActorType::KeyValue | ActorType::Writer => {
                        ROUTER_SENDER
                            .get()
                            .unwrap()
                            .send(RoutingMessage::Reset(ActorType::KeyValue))
                            .await
                            .unwrap();

                        map.remove(&ActorType::KeyValue);
                        map.remove(&ActorType::Writer);

                        break;
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let (sender, receiver) = channel(32);
    ROUTER_SENDER.set(sender).unwrap(); // setting the static sender value
    tokio::spawn(router(receiver));

    let _ = set("hello".to_string(), b"world".to_vec()).await?;
    let value = get("hello".to_string()).await?;
    println!("value: {:?}", value);

    let value = get("hello".to_string()).await?;
    println!("value: {:?}", value);

    ROUTER_SENDER
        .get()
        .unwrap()
        .send(RoutingMessage::Reset(ActorType::KeyValue))
        .await
        .unwrap();

    let value = get("hello".to_string()).await?;
    println!("value: {:?}", value);

    set("test".to_string(), b"world".to_vec()).await?;
    std::thread::sleep(std::time::Duration::from_secs(1));

    Ok(())
}
