mod staff;

use axum::{
    debug_handler,
    extract::{MatchedPath, Path, Request},
    http,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use clap::{Parser, Subcommand};
use prost::Message;
use std::collections::VecDeque;
use std::{fmt, thread};
use std::io::Cursor;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use std::{collections::HashMap, io::Read};
use axum::routing::post;

#[derive(Clone)]
struct Downstreams {
    jitter_seconds: u16,
    delay_seconds: u16,
    uris: Vec<String>,
}

impl Downstreams {
    pub fn new() -> Self {
        Downstreams {
            jitter_seconds: 14,
            delay_seconds: 30,
            uris: Vec::new(),
        }
    }
    pub fn from(j: u16, d: u16, uris: Vec<String>) -> Result<Self, String> {
        if d / 2 < j - 1 {
            Err("jitter too large".to_string())
        } else if d == 0 {
            Err("delay zero".to_string())
        } else {
            Ok(Downstreams {
                jitter_seconds: j,
                delay_seconds: d,
                uris,
            })
        }
    }

    pub fn next_run(&self, retry: u8) -> u32 {
        use rand::{thread_rng, Rng};

        let mut rng = thread_rng();
        let n: i32 = rng.gen_range((-1 * self.jitter_seconds as i32)..(self.jitter_seconds as i32));
        let delay = i32::pow(2, retry as u32) + self.delay_seconds as i32 + n;
        // Note that we will not go negative, because d/2 >= j
        1.max(delay) as u32
    }
}

struct DownstreamStatus {
    last_run_completion: std::time::Instant,
    response: HashMap<String, (u16, String)>,
}


lazy_static::lazy_static! {
    static ref topics_in_memory: Arc<Mutex<HashMap<String, VecDeque<staff::Blob>>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref downstream_addresses: Arc<RwLock<Downstreams>> = Arc::new(std::sync::RwLock::new(Downstreams::new()));
    static ref downstream_statuses:  Arc<Mutex<Option<DownstreamStatus>>> = Arc::new(Mutex::new(None));
    static ref is_forwarding: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

#[derive(Clone)]
struct Bytes {
    bytes: Vec<u8>,
}

impl From<Vec<u8>> for Bytes {
    // Copes from the incoming to the Bytes.
    fn from(value: Vec<u8>) -> Self {
        Bytes { bytes: value }
    }
}
impl Bytes {
    // Consumes the type
    pub fn as_vec(self) -> Vec<u8> {
        self.bytes
    }
}

struct HttpError {
    msg: Option<String>,
    code: http::StatusCode,
}
impl HttpError {
    fn new(msg: Option<&str>, code: http::StatusCode) -> Self {
        Self {
            msg: msg.map(|s| s.to_string()),
            code,
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.code, self.msg.unwrap_or_else(|| self.code.to_string())).into_response()
    }
}

impl From<String> for HttpError {
    fn from(s: String) -> HttpError {
        HttpError { msg: Some(s), code: http::StatusCode::INTERNAL_SERVER_ERROR}
    }
}



#[debug_handler]
async fn append_blob(
    Path(topic): Path<String>,
    payload: axum::body::Bytes,
) -> Result<&'static str, HttpError> {
    let buf = payload.to_vec();

    match staff::Blob::decode(&mut Cursor::new(buf)) {
        Ok(blob) => {
            let mut topics = topics_in_memory.lock().unwrap();
            let blobs = topics.entry(topic).or_insert(VecDeque::new());
            blobs.push_back(blob);
        }
        Err(e) => {
            return Err(HttpError::new(
                Some(e.to_string().as_str()),
                http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    }
    Ok("ok")
}

#[debug_handler]
async fn append_blobs(
    Path(topic): Path<String>,
    payload: axum::body::Bytes,
) -> Result<&'static str, HttpError> {
    let buf = payload.to_vec();

    match staff::BlobList::decode(&mut Cursor::new(buf)) {
        Ok(bloblist) => {
            eprintln!("sz: {}", bloblist.data.len());
            let mut topics = topics_in_memory.lock().unwrap();
            let blobs = topics.entry(topic).or_insert(VecDeque::new());
            let mut other: VecDeque<staff::Blob> = bloblist.data.into_iter().map(|b| b).collect();
            blobs.append(&mut other);
        }
        Err(e) => {
            return Err(HttpError::new(
                Some(e.to_string().as_str()),
                http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    }
    Ok("ok")
}

#[debug_handler]
async fn get_blob(Path(topic): Path<String>) -> Result<Vec<u8>, StatusCode> {
    let mut topics = topics_in_memory.lock().unwrap();
    let blobs = match topics.get_mut(&topic) {
        Some(blobs) => blobs,
        None => return Err(http::StatusCode::NOT_FOUND),
    };

    let response = match blobs.get(0) {
        Some(blob) => Ok(blob.encode_to_vec()),
        None => Err(http::StatusCode::NOT_FOUND),
    };

    blobs.pop_front();
    response
}

#[debug_handler]
async fn get_blobs(Path(topic): Path<String>) -> Result<Vec<u8>, StatusCode> {
    let mut topics = topics_in_memory.lock().unwrap();
    let blobs = match topics.get_mut(&topic) {
        Some(blobs) => blobs,
        None => return Err(http::StatusCode::NOT_FOUND),
    };
    //    eprintln!("topic: {}", topic);
    //    eprintln!("blobs len: {}", blobs.len());
    let data: Vec<staff::Blob> = blobs.iter().map(|b| b.clone()).collect();
    //    eprintln!("data len: {}", data.len());
    let answer = staff::BlobList { data };
    let response = Ok(answer.encode_to_vec());
    blobs.clear();

    response
}

#[debug_handler]
async fn head_blobs(Path(topic): Path<String>) -> Result<Vec<u8>, StatusCode> {
    let mut topics = topics_in_memory.lock().unwrap();
    let blobs = match topics.get_mut(&topic) {
        Some(blobs) => blobs,
        None => return Err(http::StatusCode::NOT_FOUND),
    };

    let answer = staff::TopicSize {
        length: blobs.len() as i32,
    }
    .encode_to_vec();
    Ok(answer)
}

async fn delete_blobs(Path(topic): Path<String>) -> Result<Vec<u8>, StatusCode> {
    let mut topics = topics_in_memory.lock().unwrap();
    let blobs = match topics.get_mut(&topic) {
        Some(blobs) => blobs,
        None => return Err(http::StatusCode::NOT_FOUND),
    };
    blobs.clear();
    let answer = staff::OkayEnough {}.encode_to_vec();
    Ok(answer)
}

async fn forward_blobs(payload: axum::body::Bytes,) -> Result<Vec<u8>, HttpError> {
    let buf = payload.to_vec();

    match staff::DownstreamList::decode(&mut Cursor::new(buf)) {
        Ok(list) => {

            let mut dl = downstream_addresses.write().unwrap();
            *dl = Downstreams::from(list.jitter_seconds as u16, list.delay_seconds as u16, list.target_uris)?;
        }
        Err(e) => {
            return Err(HttpError::new(
                Some(e.to_string().as_str()),
                http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    }

    Ok(staff::OkayEnough{}.encode_to_vec())
}
async fn forward_status() -> Result<Vec<u8>, StatusCode> {
    let ds = downstream_statuses.lock().unwrap();
    let ds = match &*ds {
        Some(ds) => ds,
        None => {
            return Err(http::StatusCode::NOT_FOUND);
        }
    };

    let mut targets = Vec::new();
    for (uri, (status, msg)) in &ds.response {
        targets.push(staff::Target {
            target_uri: uri.clone(),
            status: *status as u32,
            msg: msg.clone(),
        });
    }


    let is_fwd = is_forwarding.lock().unwrap();
    let answer = staff::DownstreamStatus {
        jitter_seconds: downstream_addresses.read().unwrap().jitter_seconds as u32,
        delay_seconds: downstream_addresses.read().unwrap().delay_seconds as u32,
        is_forwarding: *is_fwd,
        last_run_completion: Some(format!("{:?}", ds.last_run_completion)),
        targets,
    };

    Ok(answer.encode_to_vec())
}

async fn toggle_forwarding(payload: axum::body::Bytes) -> Result<Vec<u8>, HttpError> {
    let buf = payload.to_vec();

    match staff::SetForwarding::decode(&mut Cursor::new(buf)) {
        Ok(datum) => {
            let mut is_fwd = is_forwarding.lock().unwrap();
            *is_fwd = datum.is_forwarding;
        }
        Err(e) => {
            return Err(HttpError::new(
                Some(e.to_string().as_str()),
                http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    }

    Ok(staff::OkayEnough{}.encode_to_vec())
}

async fn log(req: Request, next: Next) -> impl IntoResponse {
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        req.uri().path().to_owned()
    };

    let method = req.method().clone();

    let start = Instant::now();
    let response = next.run(req).await;
    let latency = start.elapsed();

    let status = response.status().as_u16();

    let dt = chrono::prelude::Utc::now().to_string();

    println!(
        "{} path={} method={} status={} duration={:?}",
        dt, path, method, status, latency
    );

    response
}

#[derive(Subcommand, Debug)]
enum ClientCommands {
    /// Pushes the blob to the server. Prefixed with a @, it will read a file
    Post {
        #[arg(long, required = true)]
        topic: String,
        blob: String
    },
    /// Pushes the blobs to the server. Prefixing a string with a @, it will read a file.
    PostAll {
        #[arg(long, required = true)]
        topic: String,
        blobs: Vec<String>
    },
    /// Pops the latest from the server
    Get {
        #[arg(long, required = true)]
        topic: String,
    },
    /// Pops all the data from the server
    GetAll {
        #[arg(long, required = true)]
        topic: String,
    },
    /// How many entries left in the topic
    Head {
        #[arg(long, required = true)]
        topic: String,
    },
    /// Cleans the topic.
    Delete {
        #[arg(long, required = true)]
        topic: String,
    },
    /// Sets the list of forward
    SetForward {
        #[arg(default_value="30")]
        delay: u32,
        #[arg(default_value="15")]
        jitter: u32,
        uris: Vec<String>,
    },
    /// Gets the forwarding configuring and status of their latest PostAll
    GetForward,
    /// ToggleForwarding
    ToggleForwarding {
        #[arg(long, required = true, action=clap::ArgAction::Set)]
        forward: bool
    },
}

#[derive(Subcommand, Debug)]
enum Commands {
    Server {
        /// Address for bind - should be IP:Port
        #[arg(long, required = true)]
        server: String,
    },
    Client {
        #[arg(long, required = true)]
        server: String,
        #[command(subcommand)]
        methods: ClientCommands,
    },
}

#[derive(Debug, Parser)]
#[command(name = "staff")]
#[command(version = "0.1.0")]
#[command(about = "STore And Forward client/server system")]
#[command(propagate_version = true)]
struct Cli {
    #[arg(long, required = false)]
    verbose: Option<bool>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Debug)]
struct StaffError {}
impl StaffError {
    pub fn boxed() -> Box<StaffError> {
        Box::new(StaffError {})
    }
}
impl std::error::Error for StaffError {}

impl fmt::Display for StaffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(staff error)")
    }
}
fn collector(blob: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if blob.len() == 0 {
        return Err(StaffError::boxed());
    }
    let data: Vec<u8> = match blob.chars().nth(0).expect("path empty") {
        '-' => {
            todo!()
        }
        '@' => {
            let filename = &blob[1..];
            let mut file = match std::fs::File::open(filename) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("unable to open data file ({}) - error: {}", filename, e);
                    std::process::exit(1);
                }
            };
            let mut contents: Vec<u8> = Vec::new();
            file.read(&mut contents)?;
            contents
        }
        _ => blob.as_bytes().into(),
    };

    Ok(data)
}

fn forward_daemon() {
    loop {
        let dl = {
            // take a lock, read, drop.
            downstream_addresses.read().unwrap().clone()
        };

        let is_fwd =  { is_forwarding.lock().unwrap().clone()  };

        // when the entirety of the sending is erroring, we increase this.
        let mut total_error_count: u32 = 0;
        match is_fwd {
            true => {
                // hu hu hu this can be a bad problem!
                // need to:
                // 1. copy and drop the lock
                // count the 'number' of blobs from the front of the deque and then only clear that many
                let mut topics = topics_in_memory.lock().unwrap();
                let mut status: HashMap<String, (u16, String)> = HashMap::new();
                let mut cleared_topics = Vec::new();
                for b in topics.keys() {
                    let blobvec: Vec<staff::Blob> = { topics.get(b).unwrap().into_iter().map(|b| b.clone()).collect() };
                    let bloblist = staff::BlobList { data: blobvec };
                    let package = bloblist.encode_to_vec();
                    let mut uri_error_count = 0;
                    for uri in &dl.uris {
                        let http_client = ureq::AgentBuilder::new().build();
                        match http_client
                            .post(uri)
                            .send_bytes(&package) {
                            Ok(r) => {
                                cleared_topics.push(b.clone());
                                status.insert(uri.clone(), (r.status(), r.status_text().to_string()));
                            }
                            Err(ureq::Error::Status(code, other)) => {
                                status.insert(uri.clone(), (code, other.status_text().to_string()));
                                eprintln!("error: {} - {}", code, other.status_text());
                                uri_error_count += 1;
                                continue;
                            }
                            Err(ureq::Error::Transport(t)) => {
                                status.insert(uri.clone(), (604, t.to_string()));
                                eprintln!("error: {}", t.to_string());
                                uri_error_count += 1;
                                continue;
                            }
                        };
                    }

                    if uri_error_count == dl.uris.len() && uri_error_count > 0 {
                        total_error_count += 1;
                    } else if uri_error_count == 0 {
                        total_error_count = 0;
                    }
                }
                for b in cleared_topics {
                    topics.remove(&b);
                }
                let mut ds = downstream_statuses.lock().unwrap();
                *ds = Some(DownstreamStatus {
                    last_run_completion: Instant::now(),
                    response: HashMap::new(),
                });
            }
            false => {
                let mut ds = downstream_statuses.lock().unwrap();
                *ds = Some(DownstreamStatus {
                    last_run_completion: Instant::now(),
                    response: HashMap::new(),
                });
            }
        }

        let sleep_delay_counter = match total_error_count {
            0 => 0,
            d if d < 10 => d as u8,
            _ => 120,
        };
        let dt = chrono::prelude::Utc::now().to_string();
        let dozy = std::time::Duration::from_secs(dl.next_run(sleep_delay_counter) as u64);
        if is_fwd {
            println!("{} sleeping for {:?}", dt, dozy);
        }
        std::thread::sleep(dozy);
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Server { server } => {
            // build our application with a route
            let app = Router::new()
                .route("/",
                       get(root))
                .route("/api/v1/:topic/blob",
                       get(get_blob)
                       .post(append_blob))
                .route("/api/v1/:topic/blobs",
                       get(get_blobs)
                       .post(append_blobs)
                       .head(head_blobs)
                       .delete(delete_blobs),
                )
                .route("/api/v1/forward",
                       get(forward_status)
                       .post(forward_blobs)
                )
                .route("/api/v1/forward/toggle",
                    post(toggle_forwarding))
                .fallback(|| async {
                    http::Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body("404 Not Found".to_string())
                        .unwrap()
                })
                .route_layer(middleware::from_fn(log));

            let _ = thread::spawn(move || forward_daemon() );

            let listener = tokio::net::TcpListener::bind(server).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        }

        Commands::Client {
            server,
            methods,
        } => {
            let http_client = ureq::AgentBuilder::new().build();
            // TODO: some refactoring here is in order.
            match methods {
                ClientCommands::Post { topic, blob } => {
                    // blobs can either be _data_ themselves, can be stdin (-), or @..file
                    let data = collector(&blob).unwrap();

                    let uri = format!("http://{}/api/v1/{}/blob", server, topic);
                    let package = staff::Blob { datum: data }.encode_to_vec();
                    let _ = http_client.post(&uri).send_bytes(&package).unwrap();
                }
                ClientCommands::Get { topic } => {
                    let uri = format!("http://{}/api/v1/{}/blob", server, topic);
                    let resp: ureq::Response = match http_client.get(&uri).call() {
                        Ok(r) => r,
                        Err(ureq::Error::Transport(e)) => {
                            eprintln!("{:?}", e);
                            return;
                        }
                        Err(ureq::Error::Status(code, response)) => {
                            if code != 404 {
                                eprintln!("{:?}", response)
                            }
                            return;
                        }
                    };
                    let len: usize = resp.header("Content-Length").unwrap().parse().unwrap();
                    let mut buf: Vec<u8> = Vec::with_capacity(len);
                    resp.into_reader()
                        .take(10_000_000)
                        .read_to_end(&mut buf)
                        .unwrap();
                    match staff::Blob::decode(&mut Cursor::new(buf)) {
                        Ok(blob) => {
                            use std::io::Write;
                            let _ = std::io::stdout().write(&blob.datum);
                        }
                        Err(e) => {
                            eprint!("got error decoding result: {}", e)
                        }
                    }
                }
                ClientCommands::GetAll { topic } => {
                    let uri = format!("http://{}/api/v1/{}/blobs", server, topic);
                    let resp: ureq::Response = match http_client.get(&uri).call() {
                        Ok(r) => r,
                        Err(ureq::Error::Transport(e)) => {
                            eprintln!("{:?}", e);
                            return;
                        }
                        Err(ureq::Error::Status(_code, response)) => {
                            eprintln!("{:?}", response);
                            return;
                        }
                    };
                    let len: usize = resp.header("Content-Length").unwrap().parse().unwrap();
                    eprintln!("length: {}\n", len);
                    let mut buf: Vec<u8> = Vec::with_capacity(len);
                    resp.into_reader()
                        .take(10_000_000)
                        .read_to_end(&mut buf)
                        .unwrap();
                    match staff::BlobList::decode(&mut Cursor::new(buf)) {
                        Ok(blobs) => {
                            eprintln!("blobcount: {}", blobs.data.len());
                            for b in blobs.data {
                                use std::io::Write;
                                let _ = std::io::stdout().write(&b.datum);
                            }
                        }
                        Err(e) => {
                            eprint!("got error decoding result: {}", e)
                        }
                    }
                }
                ClientCommands::Head { topic } => {
                    let uri = format!("http://{}/api/v1/{}/blobs", server, topic);
                    let resp: ureq::Response = match http_client.head(&uri).call() {
                        Ok(r) => r,
                        Err(ureq::Error::Transport(e)) => {
                            eprintln!("{:?}", e);
                            return;
                        }
                        Err(ureq::Error::Status(_code, response)) => {
                            eprintln!("{:?}", response);
                            return;
                        }
                    };
                    let len: usize = resp.header("Content-Length").unwrap().parse().unwrap();
                    let mut buf: Vec<u8> = Vec::with_capacity(len);
                    resp.into_reader()
                        .take(10_000_000)
                        .read_to_end(&mut buf)
                        .unwrap();
                    match staff::TopicSize::decode(&mut Cursor::new(buf)) {
                        Ok(info) => {
                            println!("{}", info.length);
                        }
                        Err(e) => {
                            eprint!("got error decoding result: {}", e)
                        }
                    }

                }
                ClientCommands::PostAll { topic, blobs } => {
                    use std::sync::mpsc::{Receiver, Sender};

                    use std::sync::mpsc;
                    use std::thread;
                    let (tx, rx): (Sender<Option<Bytes>>, Receiver<Option<Bytes>>) =
                        mpsc::channel();

                    let blobs = blobs.clone();
                    let mut handles = Vec::new();
                    for b in &blobs {
                        let b2 = b.clone();
                        let thread_tx = tx.clone();
                        let jh = thread::spawn(move || {
                            match collector(&b2) {
                                Ok(data) => {
                                    let _ = thread_tx.send(Some(data.into()));
                                }
                                Err(e) => {
                                    let _ = thread_tx.send(None);
                                    eprintln!("error: {}", e);
                                }
                            };
                        });
                        handles.push(jh);
                    }

                    let mut data = Vec::with_capacity(blobs.len());
                    for _ in 0..blobs.len() {
                        let incoming = rx.recv().unwrap();
                        if let Some(datum) = incoming {
                            data.push(staff::Blob {
                                datum: datum.as_vec(),
                            });
                        }
                    }

                    let package = staff::BlobList { data: data }.encode_to_vec();

                    let uri = format!("http://{}/api/v1/{}/blobs", server, topic);
                    let _ = http_client.post(&uri).send_bytes(&package).unwrap();
                },
                ClientCommands::Delete { topic } => {
                    let uri = format!("http://{}/api/v1/{}/blob", server, topic);
                    let _ = http_client.delete(&uri).call().unwrap();
                },
                ClientCommands::SetForward { delay, jitter, uris  } => {
                    let uri = format!("http://{}/api/v1/forward", server);
                    let package = staff::DownstreamList {
                        jitter_seconds: jitter.clone(),
                        delay_seconds: delay.clone(),
                        target_uris: uris.clone(),
                    }
                    .encode_to_vec();
                    let _ = http_client.post(&uri).send_bytes(&package).unwrap();
                }

                ClientCommands::GetForward => {
                    let uri = format!("http://{}/api/v1/forward", server);
                    let resp: ureq::Response = match http_client.get(&uri).call() {
                        Ok(r) => r,
                        Err(ureq::Error::Transport(e)) => {
                            eprintln!("{:?}", e);
                            return;
                        }
                        Err(ureq::Error::Status(_code, response)) => {
                            eprintln!("{:?}", response);
                            return;
                        }
                    };
                    let len: usize = resp.header("Content-Length").unwrap().parse().unwrap();
                    let mut buf: Vec<u8> = Vec::with_capacity(len);
                    resp.into_reader()
                        .take(10_000_000)
                        .read_to_end(&mut buf)
                        .unwrap();
                    match staff::DownstreamStatus::decode(&mut Cursor::new(buf)) {
                        Ok(ds) => {
                            println!("jitter: {}", ds.jitter_seconds);
                            println!("delay: {}", ds.delay_seconds);
                            println!("forwarding: {}", ds.is_forwarding);
                            for t in ds.targets {
                                println!("target: {} status: {} msg: {}", t.target_uri, t.status, t.msg);
                            }
                        }
                        Err(e) => {
                            eprint!("got error decoding result: {}", e)
                        }
                    }

                }
                &ClientCommands::ToggleForwarding { forward } => {
                    let uri = format!("http://{}/api/v1/forward/toggle", server);
                    let package = staff::SetForwarding {
                        is_forwarding: forward,
                    }
                    .encode_to_vec();
                    let _ = http_client.post(&uri).send_bytes(&package).unwrap();
                }
            }
        }
    }
}

async fn root() -> &'static str {
    "Hello, World!"
}
