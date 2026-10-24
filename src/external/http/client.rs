use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use std::vec::Vec;

use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use super::dto::{
    HttpReqInfo, HttpReqParam, Method, PostContentType, Protocol, ResponseData, ValueSource,
};
use crate::result::Result;
use crate::variable::dto::VariableValue;

/// How many idle connections a host may keep for reuse. This caps finished
/// connections, not concurrent requests.
const POOL_MAX_IDLE_PER_HOST: usize = 8;

/// How long an idle connection is kept before it is closed. Comfortably
/// below the keep-alive of the servers and proxies these clients talk to, so
/// a reused connection is normally still alive.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on cached clients. The real key space is small — one entry
/// per distinct timeout and proxy across the settings and the flow nodes —
/// so this exists only to keep a pathological configuration from growing the
/// map for the lifetime of the process.
const MAX_CACHED_CLIENTS: usize = 32;

/// How a cached client resolves proxies.
///
/// The three states have to stay distinct: reqwest's default (read the
/// environment) is neither an explicit "no proxy" nor a configured proxy
/// URL, and a cache keyed on the timeouts alone would silently give one
/// caller another's behaviour.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ProxyMode {
    /// reqwest's default: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
    /// `NO_PROXY` — plus the Windows system proxy — are read when the client
    /// is built. What `build_req` has always used.
    Environment,
    /// `.no_proxy()`: never go through a proxy, and ignore the environment.
    /// What `get_client` gets for an empty `proxy_url`.
    Disabled,
    /// `Proxy::http(url)`: send traffic through this proxy.
    Url(String),
}

/// Everything a cached client is configured with. Two calls share a client
/// exactly when their keys are equal.
///
/// The read timeout belongs here because reqwest sets it per client and the
/// streaming callers rely on it being a timeout between reads rather than a
/// deadline for the whole response, which `RequestBuilder::timeout` is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    connect_timeout_millis: u64,
    read_timeout_millis: u64,
    proxy: ProxyMode,
}

impl ClientKey {
    /// A key in reqwest's default proxy mode — the one `build_req` uses.
    fn environment(connect_timeout_millis: u64, read_timeout_millis: u64) -> Self {
        Self {
            connect_timeout_millis,
            read_timeout_millis,
            proxy: ProxyMode::Environment,
        }
    }

    /// A key for a caller-supplied proxy URL, where an empty URL means an
    /// explicit "no proxy" rather than the environment's default.
    fn with_proxy(connect_timeout_millis: u64, read_timeout_millis: u64, proxy_url: &str) -> Self {
        Self {
            connect_timeout_millis,
            read_timeout_millis,
            proxy: if proxy_url.is_empty() {
                ProxyMode::Disabled
            } else {
                ProxyMode::Url(String::from(proxy_url))
            },
        }
    }
}

/// Build the client a key describes. This is the expensive half — the
/// `native-tls` backend loads the platform trust store here — so it runs on
/// a cache miss only, and with no lock held.
fn build_client(key: &ClientKey) -> reqwest::Result<Client> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_millis(key.connect_timeout_millis))
        .read_timeout(Duration::from_millis(key.read_timeout_millis))
        // Clients are cached now, so their pools are worth keeping: idle
        // connections survive between calls to the same host, which is what
        // saves the TCP and TLS handshake on every request.
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT);
    builder = match &key.proxy {
        ProxyMode::Environment => builder,
        ProxyMode::Disabled => builder.no_proxy(),
        ProxyMode::Url(url) => builder.proxy(reqwest::Proxy::http(url)?),
    };
    builder.build()
}

/// The cached clients and the insertion order used to evict from them,
/// behind one lock so the two can never disagree.
struct CacheInner {
    clients: HashMap<ClientKey, Client>,
    order: VecDeque<ClientKey>,
}

/// A process-wide cache of `reqwest::Client`s, keyed by the parts of the
/// configuration that cannot be varied per request.
///
/// A `Client` is an `Arc` inside, so a hit hands back a clone that shares its
/// connection pool with every other holder: cheaper than building, and the
/// connections are reused instead of re-handshaken. Two different keys never
/// share a pool, which is what lets callers with different proxies — or
/// different timeouts — use one cache without affecting each other.
///
/// The lock is held only for map and queue bookkeeping: never across an
/// await, and never while a client is built.
struct ClientCache {
    inner: Mutex<CacheInner>,
    max_entries: usize,
    build: fn(&ClientKey) -> reqwest::Result<Client>,
}

impl ClientCache {
    /// A cache holding at most `max_entries` clients, built by `build`. The
    /// builder is a parameter so a unit test can count builds instead of
    /// touching the network.
    fn with_builder(max_entries: usize, build: fn(&ClientKey) -> reqwest::Result<Client>) -> Self {
        debug_assert!(max_entries > 0, "a cache with no room caches nothing");
        Self {
            inner: Mutex::new(CacheInner {
                clients: HashMap::with_capacity(max_entries),
                order: VecDeque::with_capacity(max_entries),
            }),
            max_entries,
            build,
        }
    }

    /// The client for `key`, built and cached on first use.
    fn get(&self, key: &ClientKey) -> reqwest::Result<Client> {
        if let Some(client) = self.lookup(key) {
            return Ok(client);
        }
        // Built outside the lock: a slow or failing build must not hold up
        // every other request's lookup.
        let client = (self.build)(key)?;
        Ok(self.insert(key.clone(), client))
    }

    fn lookup(&self, key: &ClientKey) -> Option<Client> {
        self.lock().clients.get(key).cloned()
    }

    /// Store `client` under `key` and return the client now stored. If
    /// another thread stored the same key first, its client wins and the one
    /// just built is dropped unused: one wasted build, nothing else.
    fn insert(&self, key: ClientKey, client: Client) -> Client {
        let mut inner = self.lock();
        if let Some(existing) = inner.clients.get(&key) {
            return existing.clone();
        }
        // Evict oldest-inserted first, so the map stays bounded however many
        // distinct timeouts or proxies a configuration produces.
        if inner.clients.len() >= self.max_entries
            && let Some(oldest) = inner.order.pop_front()
        {
            inner.clients.remove(&oldest);
        }
        inner.order.push_back(key.clone());
        inner.clients.insert(key, client.clone());
        client
    }

    /// No panicking work runs under the lock, so the most a poison can lose
    /// is a cache entry — taking the map back beats failing an unrelated
    /// request over it.
    fn lock(&self) -> MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of cached clients.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().clients.len()
    }

    /// Whether `key` is cached.
    #[cfg(test)]
    fn contains(&self, key: &ClientKey) -> bool {
        self.lock().clients.contains_key(key)
    }
}

/// Every HTTP client this crate builds, shared by every request.
static HTTP_CLIENTS: LazyLock<ClientCache> =
    LazyLock::new(|| ClientCache::with_builder(MAX_CACHED_CLIENTS, build_client));

/// The `reqwest::Client` for this configuration, from the process-wide cache.
///
/// A client is built once per distinct (connect timeout, read timeout, proxy)
/// triple and then shared, so the platform trust store is loaded once and
/// keep-alive connections are reused across calls. An empty `proxy_url` means
/// `.no_proxy()` and ignores the environment, exactly as before.
pub(crate) fn get_client(
    connect_timeout_millis: u64,
    read_timeout_millis: u64,
    proxy_url: &str,
) -> Result<Client> {
    let key = ClientKey::with_proxy(connect_timeout_millis, read_timeout_millis, proxy_url);
    Ok(HTTP_CLIENTS.get(&key)?)
}

pub(crate) async fn status_code(
    info: HttpReqInfo,
    connect_timeout_milliseconds: u64,
    read_timeout_milliseconds: u64,
    vars: HashMap<String, VariableValue>,
) -> Result<u16> {
    let req = build_req(
        &info,
        connect_timeout_milliseconds,
        read_timeout_milliseconds,
        &vars,
    )?;
    let res = req.send().await?;
    Ok(res.status().as_u16())
}

pub(crate) async fn req(
    info: HttpReqInfo,
    connect_timeout_milliseconds: u64,
    read_timeout_milliseconds: u64,
    vars: &HashMap<String, VariableValue>,
) -> reqwest::Result<ResponseData> {
    let req = build_req(&info, connect_timeout_milliseconds, read_timeout_milliseconds, vars)?;
    let res = req.send().await?;
    // println!("http status code {}", res.status().as_str());
    if res.status() != reqwest::StatusCode::OK {
        return Ok(ResponseData::None);
    }
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map_or("", |h| h.to_str().unwrap());
    let data = if content_type.contains("text/")
        || content_type.contains("/json")
        || content_type.contains("/xml")
    {
        let s = res.text().await?;
        // println!("{}", s);
        ResponseData::Str(s)
    } else {
        ResponseData::Bin(res.bytes().await?.to_vec())
    };
    Ok(data)
}

/// The request body as the wire should carry it: the rich text the editor
/// stores (variable chips) reduced to plain text, with the values of every
/// referenced variable filled in — the same `vars` the headers and query
/// parameters resolve against. An unknown variable is left as the `` `name` ``
/// it was written as.
fn resolve_body(body: &str, vars: &HashMap<String, VariableValue>) -> String {
    let plain = crate::flow::rt::var_replace::rich_text_body_to_plain(body);
    match crate::flow::rt::var_replace::replace_vars_with(&plain, |name| {
        Ok(vars.get(name).map(|v| v.val_to_string()))
    }) {
        Ok(s) => s,
        // The resolver above never fails; keep the plain text either way.
        Err(_) => plain,
    }
}

/// The current value of a parameter: the literal it holds, or the value of
/// the variable it is sourced from — an empty string when that variable is
/// not set. Headers, query parameters and form data all resolve this way.
fn param_value(param: &HttpReqParam, vars: &HashMap<String, VariableValue>) -> String {
    match param.value_source {
        ValueSource::Val => param.value.clone(),
        ValueSource::Var => vars
            .get(&param.value)
            .map_or(String::new(), |v| v.val_to_string()),
    }
}

/// Turn a parameter table into the `(name, value)` pairs reqwest encodes, for
/// a query string or a form body alike.
fn pairs<'a>(
    params: &'a [HttpReqParam],
    vars: &HashMap<String, VariableValue>,
) -> Vec<(&'a str, String)> {
    params
        .iter()
        .map(|p| (p.name.as_str(), param_value(p, vars)))
        .collect()
}

/// Turn a parameter table into the headers reqwest will send, skipping any
/// pair it cannot represent.
///
/// A header name comes from the configuration table and a variable-sourced
/// value comes from runtime state, so either can hold something HTTP does not
/// allow. Dropping the offending pair and logging it keeps the request, and
/// the thread serving it, alive; the remaining headers still go out.
fn header_map(
    params: &[HttpReqParam],
    vars: &HashMap<String, VariableValue>,
) -> HeaderMap<HeaderValue> {
    let mut headers = HeaderMap::with_capacity(params.len());
    for p in params {
        let name = match HeaderName::from_bytes(p.name.as_bytes()) {
            Ok(name) => name,
            Err(e) => {
                log::warn!("Skipping HTTP header `{}`: {e}", p.name);
                continue;
            }
        };
        match param_value(p, vars).parse::<HeaderValue>() {
            Ok(value) => {
                headers.insert(name, value);
            }
            Err(e) => log::warn!("Skipping HTTP header `{}`: {e}", p.name),
        }
    }
    headers
}

fn build_req(
    info: &HttpReqInfo,
    connect_timeout_milliseconds: u64,
    timeout_milliseconds: u64,
    vars: &HashMap<String, VariableValue>,
) -> reqwest::Result<RequestBuilder> {
    // The caller's connect and read timeouts, on a shared client, so its pool
    // is reused between requests. In reqwest's default proxy mode, i.e. the
    // environment decides, which is what this path has always done.
    let client = HTTP_CLIENTS.get(&ClientKey::environment(
        connect_timeout_milliseconds,
        timeout_milliseconds,
    ))?;
    let mut url = String::with_capacity(512);
    match info.protocol {
        Protocol::HTTP => url.push_str("http"),
        Protocol::HTTPS => url.push_str("https"),
    }
    url.push_str("://");
    url.push_str(&info.address);
    let mut req = match info.method {
        Method::GET => client.get(&url),
        Method::POST => {
            let r = client.post(&url);
            if info.post_content_type == PostContentType::UrlEncoded {
                // The body is the parameter table. `request_body` belongs to
                // RAW and is not editable in this mode.
                if info.form_data.is_empty() {
                    r
                } else {
                    r.form(&pairs(&info.form_data, vars))
                }
            } else {
                let body = resolve_body(&info.request_body, vars);
                if body.is_empty() { r } else { r.body(body) }
            }
        }
    };
    if !info.headers.is_empty() {
        req = req.headers(header_map(&info.headers, vars));
    }
    if !info.query_params.is_empty() {
        req = req.query(&pairs(&info.query_params, vars));
    }
    if info.post_content_type == PostContentType::Raw {
        // The body is whatever the user typed. Only the default content type
        // assumes JSON; an empty `content_type` is a record saved before the
        // field existed, and those were sent as application/json.
        let content_type = info.content_type.trim();
        req = req.header(
            "Content-Type",
            if content_type.is_empty() {
                "application/json"
            } else {
                content_type
            },
        );
    }
    if !info.user_agent.is_empty() {
        req = req.header("User-Agent", &info.user_agent);
    }
    // Ok(req.timeout(Duration::from_millis(info.timeout_milliseconds)))
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variable::dto::VariableType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CONNECT_TIMEOUT_MILLIS: u64 = 1_000;

    fn vars() -> HashMap<String, VariableValue> {
        let mut vars = HashMap::new();
        vars.insert(
            String::from("name"),
            VariableValue::new("Ada", &VariableType::Str),
        );
        vars.insert(
            String::from("count"),
            VariableValue::new("7", &VariableType::Num),
        );
        vars
    }

    #[test]
    fn pairs_resolve_variables() {
        let p = |name: &str, value: &str, source: ValueSource| HttpReqParam {
            name: String::from(name),
            value: String::from(value),
            value_source: source,
        };
        let params = vec![
            p("a", "x", ValueSource::Val),
            p("b", "name", ValueSource::Var),
            // Known variable of another kind, and one that does not exist.
            p("c", "count", ValueSource::Var),
            p("d", "nope", ValueSource::Var),
        ];
        assert_eq!(
            pairs(&params, &vars()),
            vec![
                ("a", String::from("x")),
                ("b", String::from("Ada")),
                ("c", String::from("7")),
                ("d", String::new()),
            ]
        );
    }

    #[test]
    fn body_from_editor_is_plain_and_substituted() {
        let body = "<p>{\"name\": \"<var class=\"var-chip\" data-var-name=\"name\" data-var-type=\"String\">name</var>\", \"count\": <var data-var-name=\"count\">count</var>}</p>";
        assert_eq!(
            resolve_body(body, &vars()),
            "{\"name\": \"Ada\", \"count\": 7}\n"
        );
    }

    #[test]
    fn body_keeps_unknown_variable() {
        let body = "<p><var data-var-name=\"nope\">nope</var></p>";
        assert_eq!(resolve_body(body, &vars()), "`nope`\n");
    }

    #[test]
    fn legacy_plain_body_is_substituted() {
        assert_eq!(
            resolve_body("{\"name\": \"`name`\"}", &vars()),
            "{\"name\": \"Ada\"}"
        );
        // …and text without any variable reference is sent as written.
        assert_eq!(
            resolve_body("{\"a\": \"a<b\"}", &vars()),
            "{\"a\": \"a<b\"}"
        );
    }

    #[test]
    fn header_map_skips_pairs_http_cannot_represent() {
        let p = |name: &str, value: &str, source: ValueSource| HttpReqParam {
            name: String::from(name),
            value: String::from(value),
            value_source: source,
        };
        let params = vec![
            p("X-Token", "literal", ValueSource::Val),
            p("X-Name", "name", ValueSource::Var),
            // A space is not allowed in a header name and a newline is not
            // allowed in a value, so neither pair can be sent.
            p("X Bad", "v", ValueSource::Val),
            p("X-Newline", "a\nb", ValueSource::Val),
        ];
        let headers = header_map(&params, &vars());
        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get("X-Token").unwrap(), "literal");
        assert_eq!(headers.get("X-Name").unwrap(), "Ada");
        assert!(headers.get("X-Newline").is_none());
    }

    // Each cache test builds its own `ClientCache` rather than reaching for
    // the shared `HTTP_CLIENTS`: the test harness runs these concurrently in
    // one process, so anything global would make the assertions order
    // dependent. `build_client` builds a client without sending a request,
    // so none of them touch the network.

    /// The claim the cache exists for: one client per configuration, handed
    /// out again rather than rebuilt, and a different read timeout is a
    /// different configuration.
    #[test]
    fn client_cache_reuses_one_client_per_key() {
        static BUILDS: AtomicUsize = AtomicUsize::new(0);
        fn build(key: &ClientKey) -> reqwest::Result<Client> {
            BUILDS.fetch_add(1, Ordering::SeqCst);
            build_client(key)
        }
        let cache = ClientCache::with_builder(4, build);
        let key = ClientKey::environment(CONNECT_TIMEOUT_MILLIS, 5_000);
        assert!(cache.get(&key).is_ok());
        assert!(cache.get(&key).is_ok());
        assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
        assert_eq!(cache.len(), 1);

        let other = ClientKey::environment(CONNECT_TIMEOUT_MILLIS, 9_000);
        assert!(cache.get(&other).is_ok());
        assert_eq!(BUILDS.load(Ordering::SeqCst), 2);
        assert_eq!(cache.len(), 2);
    }

    /// The three proxy states have to stay apart: `build_req` has always let
    /// the environment decide, `get_client` has always ignored it when no
    /// proxy is configured. Collapsing them would change one caller's
    /// behaviour to the other's.
    #[test]
    fn environment_and_no_proxy_are_different_clients() {
        let environment = ClientKey::environment(1_000, 5_000);
        let disabled = ClientKey::with_proxy(1_000, 5_000, "");
        let url = ClientKey::with_proxy(1_000, 5_000, "http://127.0.0.1:7890");
        assert_ne!(environment, disabled);
        assert_ne!(environment, url);
        assert_ne!(disabled, url);
    }

    /// The oldest client goes first, so a process that sees many distinct
    /// timeouts or proxies still caches a bounded number of them.
    #[test]
    fn client_cache_evicts_the_oldest_entry() {
        let cache = ClientCache::with_builder(2, build_client);
        let first = ClientKey::environment(CONNECT_TIMEOUT_MILLIS, 1_000);
        let second = ClientKey::environment(CONNECT_TIMEOUT_MILLIS, 2_000);
        let third = ClientKey::environment(CONNECT_TIMEOUT_MILLIS, 3_000);
        for key in [&first, &second, &third] {
            assert!(cache.get(key).is_ok());
        }
        assert_eq!(cache.len(), 2);
        assert!(!cache.contains(&first));
        assert!(cache.contains(&second));
        assert!(cache.contains(&third));
    }

    /// A client that cannot be built is reported every time, as it was
    /// before the cache existed, and is not remembered as a failure.
    #[test]
    fn a_bad_proxy_url_fails_and_is_not_cached() {
        let cache = ClientCache::with_builder(4, build_client);
        let key = ClientKey::with_proxy(1_000, 2_000, "not a url");
        assert!(cache.get(&key).is_err());
        assert!(cache.get(&key).is_err());
        assert_eq!(cache.len(), 0);
    }

    /// `get_client` has to reach the shared cache, not a cache of its own: the
    /// client for a configuration has to end up in `HTTP_CLIENTS`, where the
    /// next caller with those timeouts finds it.
    ///
    /// Nothing here counts entries: `HTTP_CLIENTS` is process-wide and other
    /// tests in this binary reach it too — `ai::chat`'s provider tests call
    /// `get_client` with timeouts of their own — and libtest runs them
    /// concurrently, so entries can appear between any two reads. Presence is
    /// the part no other test can make true: no one else uses these numbers.
    /// That the cache builds each key once and then reuses it is settled
    /// without racing by `client_cache_reuses_one_client_per_key`, which counts
    /// builds on a cache of its own.
    #[test]
    fn get_client_goes_through_the_shared_cache() {
        let key = ClientKey::with_proxy(1_000, 5_000, "");
        assert!(get_client(1_000, 5_000, "").is_ok());
        assert!(HTTP_CLIENTS.contains(&key), "get_client did not cache it");
        assert!(get_client(1_000, 5_000, "").is_ok());
        assert!(HTTP_CLIENTS.contains(&key), "the cached client went away");
    }

    /// The point of the cache, end to end: two `get_client` calls with the
    /// same configuration hand out clients that reach the server over one
    /// TCP connection, so the handshake is paid once instead of per request.
    ///
    /// A server is needed because a client's pool is not observable from the
    /// outside; counting the connections it opens is. Building the clients
    /// directly instead of asking the cache for them makes this fail with
    /// two connections, which is what the old code did on every request.
    #[tokio::test]
    async fn get_client_calls_share_one_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Answers every request on a connection without closing it, so the
        // client is free to send the next one over the same socket. Left
        // running until the test process exits.
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                CONNECTIONS.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    // A GET carries no body, so the blank line ends it.
                    let head_end = |buf: &[u8]| {
                        buf.windows(4)
                            .position(|w| w == b"\r\n\r\n")
                            .map(|i| i + 4)
                    };
                    let mut unread = Vec::new();
                    let mut chunk = [0u8; 512];
                    loop {
                        let n = socket.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        unread.extend_from_slice(&chunk[..n]);
                        while let Some(end) = head_end(&unread) {
                            unread.drain(..end);
                            if socket
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                });
            }
        });

        let url = format!("http://{addr}/");
        // A timeout no other test uses, so the shared cache stays out of
        // this test's way.
        let a = get_client(1_000, 7_777, "").unwrap();
        let b = get_client(1_000, 7_777, "").unwrap();
        assert_eq!(a.get(&url).send().await.unwrap().text().await.unwrap(), "ok");
        assert_eq!(b.get(&url).send().await.unwrap().text().await.unwrap(), "ok");
        assert_eq!(
            CONNECTIONS.load(Ordering::SeqCst),
            1,
            "the second request should have reused the first connection"
        );
    }

    /// The environment mode must leave reqwest's proxy detection alone —
    /// adding `.no_proxy()` there would silently stop honouring
    /// `HTTPS_PROXY` for every HTTP call node.
    #[test]
    fn every_proxy_mode_builds() {
        for key in [
            ClientKey::environment(1_000, 2_000),
            ClientKey::with_proxy(1_000, 2_000, ""),
            ClientKey::with_proxy(1_000, 2_000, "http://127.0.0.1:7890"),
        ] {
            assert!(build_client(&key).is_ok(), "{key:?} should build");
        }
    }
}
