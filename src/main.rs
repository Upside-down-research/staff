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
use std::fmt;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{collections::HashMap, io::Read};
use axum::routing::post;

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
    last_run: std::time::Instant,
    response: HashMap<String, (u32, String)>,
}


lazy_static::lazy_static! {
    static ref topics_in_memory: Arc<Mutex<HashMap<String, VecDeque<staff::Blob>>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref downstream_addresses: Arc<Mutex<Downstreams>> = Arc::new(Mutex::new(Downstreams::new()));
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

            let mut dl = downstream_addresses.lock().unwrap();
            *dl = Downstreams::from(list.jitter_seconds as u16, list.delay_seconds as u16, list.targets)?;
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
    todo!()
}

async fn toggle_forwarding(payload: axum::body::Bytes) -> Result<Vec<u8>, HttpError> {
    let is_fwd = is_forwarding.lock().unwrap();
    *is_fwd = !*is_fwd;
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
    Post { blob: String },
    /// Pushes the blobs to the server. Prefixing a string with a @, it will read a file.
    PostAll { blobs: Vec<String> },
    /// Pops the latest from the server
    Get,
    /// Pops all the data from the server
    GetAll,
    /// How many entries left in the topic
    Head,
    /// Cleans the topic.
    Delete,
    /// Sets the list of forward
    SetForward {
        #[arg(default_value="30")]
        delay: u16,
        #[arg(default_value="15")]
        jitter: u16,
        uris: Vec<String>,
    },
    /// Gets the forwarding configuring and status of their latest PostAll
    GetForward,
    /// ToggleForwarding
    ToggleForwarding,
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
        #[arg(long, required = true)]
        topic: String,
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
                .route("/api/v1/forward/enable",
                    post(toggle_forwarding))
                .fallback(|| async {
                    http::Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body("404 Not Found".to_string())
                        .unwrap()
                })
                .route_layer(middleware::from_fn(log));

            let listener = tokio::net::TcpListener::bind(server).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        }

        Commands::Client {
            server,
            topic,
            methods,
        } => {
            let http_client = ureq::AgentBuilder::new().build();
            // TODO: some refactoring here is in order.
            match methods {
                ClientCommands::Post { blob } => {
                    // blobs can either be _data_ themselves, can be stdin (-), or @..file
                    let data = collector(&blob).unwrap();

                    let uri = format!("http://{}/api/v1/{}/blob", server, topic);
                    let package = staff::Blob { datum: data }.encode_to_vec();
                    let _ = http_client.post(&uri).send_bytes(&package).unwrap();
                }
                ClientCommands::Get => {
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
                ClientCommands::GetAll => {
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
                ClientCommands::Head => {
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
                ClientCommands::PostAll { blobs } => {
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
                &ClientCommands::Delete => {
                    let uri = format!("http://{}/api/v1/{}/blob", server, topic);
                    let _ = http_client.delete(&uri).call().unwrap();
                },
                &ClientCommands::SetForward { .. } | &ClientCommands::GetForward => todo!()

            }
        }
    }
}

async fn root() -> &'static str {
    "Hello, World!"
}
