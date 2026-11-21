//! Proxy auto-config (Options → Proxy / Socks → "Use automatic configuration script"): the
//! script's `FindProxyForURL(url, host)` decides the proxy for each address. It runs in a small
//! JavaScript engine (QuickJS), with the helper functions PAC scripts expect written here (our
//! own, from the PAC definition) and DNS answered by the system. An answer is kept per script and
//! the address the script was given (a script decides by path, scheme or port too, not only by
//! host; an https address is cut to `https://host:port/`, as browsers give it one). The script
//! comes from the network: it gets the language only (no files, no modules), limited memory and a
//! limited time.

use std::cell::Cell;
use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::prelude::{Coerced, Opt};
use rquickjs::{CatchResultExt, Context, Function, Runtime, Value};
use url::Url;

/// Where a request goes, in the script's order of preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    /// A proxy address: `http://host:port`, `https://…`, `socks5h://…` or `socks4://…`.
    Proxy(String),
}

/// The routes for `url` under `script`: its answer's entries that MDM can use, in order (the
/// first is taken). Err if the script doesn't run.
pub fn routes(script: &Arc<str>, url: &Url) -> Result<Vec<Route>, String> {
    use std::hash::{Hash, Hasher};
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    // What the script is given (and answers for): an https address is cut to its scheme, host
    // and port, as browsers hand it one, so a path or query (a token, a private page) can't
    // reach the script. Kept per script (its text's hash) and that address, since a script may
    // answer by path, scheme or port as well as by host.
    let asked = script_address(url);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    script.hash(&mut h);
    let key = (h.finish(), asked.clone());
    if let Some(r) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return Ok(r.clone());
    }
    let answer = run(script, asked.as_str(), &host)?;
    let r = parse(&answer);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    // A page of thousands of links, each with its own answer: start again rather than grow
    // without end (an answer is a script run away).
    if cache.len() >= CACHE_MAX {
        cache.clear();
    }
    cache.insert(key, r.clone());
    Ok(r)
}

/// The address a PAC script is given: browsers hand an https address as `https://host:port/`
/// with no path, query or fragment (an http one goes as it is).
fn script_address(url: &Url) -> Url {
    let mut asked = url.clone();
    if asked.scheme() == "https" {
        asked.set_path("/");
        asked.set_query(None);
        asked.set_fragment(None);
    }
    asked
}

type Answers = HashMap<(u64, Url), Vec<Route>>;
static CACHE: Mutex<Option<Answers>> = Mutex::new(None);
/// The most answers kept at once.
const CACHE_MAX: usize = 4096;

/// `FindProxyForURL`'s answer: `PROXY a:8080; SOCKS b:1080; DIRECT` → routes, unknown kinds
/// skipped. An empty answer means direct.
pub fn parse(answer: &str) -> Vec<Route> {
    let mut out: Vec<Route> = answer
        .split(';')
        .filter_map(|entry| {
            let mut it = entry.split_whitespace();
            let kind = it.next()?.to_ascii_uppercase();
            let at = it.next();
            match (kind.as_str(), at) {
                ("DIRECT", _) => Some(Route::Direct),
                ("PROXY" | "HTTP", Some(a)) => Some(Route::Proxy(format!("http://{a}"))),
                ("HTTPS", Some(a)) => Some(Route::Proxy(format!("https://{a}"))),
                // A SOCKS5 proxy looks the site's name up, as browsers have it (#1558):
                // this computer's DNS never sees it.
                ("SOCKS" | "SOCKS5", Some(a)) => Some(Route::Proxy(format!("socks5h://{a}"))),
                ("SOCKS4", Some(a)) => Some(Route::Proxy(format!("socks4://{a}"))),
                _ => None,
            }
        })
        .collect();
    if out.is_empty() {
        out.push(Route::Direct);
    }
    out
}

/// What a script may take: memory (a PAC script needs well under 1 MB), stack (deep recursion
/// stops there, short of the thread's own) and running time (the system's DNS answers not
/// counted, up to `DNS_LIMIT` of them: a script that keeps looking up slow names still ends).
/// Past one it stops with an error.
const MEMORY_LIMIT: usize = 32 << 20;
const STACK_LIMIT: usize = 512 << 10;
const TIME_LIMIT: Duration = Duration::from_secs(2);
const DNS_LIMIT: Duration = Duration::from_secs(30);

/// Run the script and call `FindProxyForURL(url, host)`.
fn run(script: &str, url: &str, host: &str) -> Result<String, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_LIMIT);
    // QuickJS asks now and then while the script runs; a DNS lookup moves the end on by the
    // time it took, never past `last`.
    let begun = Instant::now();
    let last = begun + TIME_LIMIT + DNS_LIMIT;
    let end = Rc::new(Cell::new(begun + TIME_LIMIT));
    let over = end.clone();
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > over.get())));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    ctx.with(|ctx| {
        let find = (|| {
            let global = ctx.globals();
            let lookup = move |host: Opt<Coerced<String>>| {
                let start = Instant::now();
                let ip = resolve(&host.0.map(|h| h.0).unwrap_or_default());
                end.set((end.get() + start.elapsed()).min(last));
                ip
            };
            global.set("__mdmResolve", Function::new(ctx.clone(), lookup)?)?;
            global.set("__mdmMyIp", Function::new(ctx.clone(), my_ip)?)?;
            ctx.eval::<(), _>(HELPERS)?;
            ctx.eval::<(), _>(script)?;
            global.get::<_, Value>("FindProxyForURL")
        })()
        .catch(&ctx)
        .map_err(|e| e.to_string())?;
        let find = find
            .into_function()
            .ok_or_else(|| "the script has no FindProxyForURL".to_owned())?;
        find.call::<_, Coerced<String>>((url, host))
            .catch(&ctx)
            .map(|answer| answer.0)
            .map_err(|e| e.to_string())
    })
}

/// `__mdmResolve(host)`: the host's first IPv4 address, or "".
fn resolve(host: &str) -> String {
    (host, 0)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.find(|s| s.is_ipv4()))
        .map(|s| s.ip().to_string())
        .unwrap_or_default()
}

/// `__mdmMyIp()`: this computer's address on its network (the one a connection out would use;
/// nothing is sent), else 127.0.0.1.
fn my_ip() -> String {
    crate::net::source_address(IpAddr::from([192, 0, 2, 1]))
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
        .to_string()
}

/// The functions PAC scripts may call (as browsers provide them).
const HELPERS: &str = r#"
function isPlainHostName(host) { return String(host).indexOf('.') < 0; }
function dnsDomainIs(host, domain) {
  host = String(host).toLowerCase(); domain = String(domain).toLowerCase();
  return host.length >= domain.length && host.substring(host.length - domain.length) === domain;
}
function localHostOrDomainIs(host, hostdom) {
  host = String(host).toLowerCase(); hostdom = String(hostdom).toLowerCase();
  return host === hostdom || (host.indexOf('.') < 0 && hostdom.indexOf(host + '.') === 0);
}
function dnsResolve(host) { var a = __mdmResolve(String(host)); return a === '' ? null : a; }
function isResolvable(host) { return dnsResolve(host) !== null; }
function myIpAddress() { return __mdmMyIp(); }
function dnsDomainLevels(host) { return String(host).split('.').length - 1; }
function __mdmAddr(s) {
  var p = String(s).split('.');
  if (p.length !== 4) return null;
  var n = 0;
  for (var i = 0; i < 4; i++) {
    if (!/^\d+$/.test(p[i])) return null;
    var x = parseInt(p[i], 10);
    if (x > 255) return null;
    n = n * 256 + x;
  }
  return n;
}
function convert_addr(ipchars) { var n = __mdmAddr(ipchars); return n === null ? 0 : n; }
function isInNet(host, pattern, mask) {
  var ip = __mdmAddr(host) !== null ? host : dnsResolve(host);
  if (ip === null) return false;
  var a = __mdmAddr(ip), p = __mdmAddr(pattern), m = __mdmAddr(mask);
  if (a === null || p === null || m === null) return false;
  return ((a & m) >>> 0) === ((p & m) >>> 0);
}
function shExpMatch(str, exp) {
  var re = String(exp).replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.');
  return new RegExp('^' + re + '$').test(String(str));
}
var __mdmDays = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'];
var __mdmMonths = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];
function __mdmGmt(args) { return args.length > 0 && String(args[args.length - 1]).toUpperCase() === 'GMT'; }
function __mdmIn(v, lo, hi) { return lo <= hi ? (v >= lo && v <= hi) : (v >= lo || v <= hi); }
function weekdayRange() {
  var a = Array.prototype.slice.call(arguments), gmt = __mdmGmt(a);
  if (gmt) a.pop();
  var now = new Date(), d = gmt ? now.getUTCDay() : now.getDay();
  var d1 = __mdmDays.indexOf(String(a[0]).toUpperCase());
  var d2 = a.length > 1 ? __mdmDays.indexOf(String(a[1]).toUpperCase()) : d1;
  if (d1 < 0 || d2 < 0) return false;
  return __mdmIn(d, d1, d2);
}
function timeRange() {
  var a = Array.prototype.slice.call(arguments), gmt = __mdmGmt(a);
  if (gmt) a.pop();
  a = a.map(function (x) { return parseInt(x, 10); });
  var now = new Date();
  var h = gmt ? now.getUTCHours() : now.getHours();
  var m = gmt ? now.getUTCMinutes() : now.getMinutes();
  var s = gmt ? now.getUTCSeconds() : now.getSeconds();
  var t = h * 3600 + m * 60 + s;
  if (a.length === 1) return h === a[0];
  if (a.length === 2) return __mdmIn(h, a[0], a[1]);
  if (a.length === 4) return __mdmIn(t, a[0] * 3600 + a[1] * 60, a[2] * 3600 + a[3] * 60 + 59);
  if (a.length === 6) return __mdmIn(t, a[0] * 3600 + a[1] * 60 + a[2], a[3] * 3600 + a[4] * 60 + a[5]);
  return false;
}
function dateRange() {
  var a = Array.prototype.slice.call(arguments), gmt = __mdmGmt(a);
  if (gmt) a.pop();
  var now = new Date();
  var cur = { d: gmt ? now.getUTCDate() : now.getDate(), m: gmt ? now.getUTCMonth() : now.getMonth(),
              y: gmt ? now.getUTCFullYear() : now.getFullYear() };
  // Each argument: a month name, a year (over 31) or a day.
  var parts = a.map(function (x) {
    var i = __mdmMonths.indexOf(String(x).toUpperCase());
    if (i >= 0) return { k: 'm', v: i };
    var n = parseInt(x, 10);
    return n > 31 ? { k: 'y', v: n } : { k: 'd', v: n };
  });
  if (parts.length === 1) return cur[parts[0].k] === parts[0].v;
  // The kinds given decide what is compared; each end is today with those parts set.
  var kinds = {};
  parts.forEach(function (p) { kinds[p.k] = true; });
  var key = function (x) { return (kinds.y ? x.y : 0) * 10000 + (kinds.m ? x.m : 0) * 100 + (kinds.d ? x.d : 0); };
  var end = function (ps) {
    var x = { y: cur.y, m: cur.m, d: cur.d };
    ps.forEach(function (p) { x[p.k] = p.v; });
    return x;
  };
  var half = parts.length / 2;
  return __mdmIn(key(cur), key(end(parts.slice(0, half))), key(end(parts.slice(half))));
}
function alert() {}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn route(script: &str, url: &str) -> Vec<Route> {
        routes(&Arc::from(script), &Url::parse(url).unwrap()).unwrap()
    }

    #[test]
    fn answers_become_routes() {
        assert_eq!(
            parse("PROXY proxy.corp:8080; SOCKS s.corp:1080; DIRECT"),
            vec![
                Route::Proxy("http://proxy.corp:8080".into()),
                Route::Proxy("socks5h://s.corp:1080".into()),
                Route::Direct
            ]
        );
        assert_eq!(
            parse("SOCKS5 s.corp:1080; SOCKS4 old.corp:1080"),
            vec![
                Route::Proxy("socks5h://s.corp:1080".into()),
                Route::Proxy("socks4://old.corp:1080".into()),
            ]
        );
        assert_eq!(parse(""), vec![Route::Direct]);
        assert_eq!(parse("WEIRD x; DIRECT"), vec![Route::Direct]);
    }

    /// A script may answer by path: one address's answer must not decide another's on the same
    /// host. (The answer kept was per host, so the first address's proxy went to them all.)
    #[test]
    fn an_answer_is_kept_per_address_not_per_host() {
        let script: Arc<str> = Arc::from(
            r#"
            function FindProxyForURL(url, host) {
                return url.indexOf("/proxy/") >= 0 ? "PROXY p.test:8080" : "DIRECT";
            }"#,
        );
        assert_eq!(
            routes(&script, &Url::parse("http://x.test/proxy/a.bin").unwrap()).unwrap(),
            vec![Route::Proxy("http://p.test:8080".into())]
        );
        assert_eq!(
            routes(&script, &Url::parse("http://x.test/direct/b.bin").unwrap()).unwrap(),
            vec![Route::Direct],
            "another address on the same host has its own answer"
        );
    }

    /// An https address reaches the script as `https://host:port/`, no path, query or fragment,
    /// as browsers hand it one: a token or a private page can't get out through the script.
    #[test]
    fn an_https_address_is_cut_before_the_script_sees_it() {
        let script: Arc<str> = Arc::from(
            r#"
            function FindProxyForURL(url, host) {
                return url === "https://x.test:8443/" ? "DIRECT" : "PROXY p.test:8080";
            }"#,
        );
        assert_eq!(
            routes(
                &script,
                &Url::parse("https://x.test:8443/private/a.bin?token=1#top").unwrap()
            )
            .unwrap(),
            vec![Route::Direct],
            "the path, query and fragment never reach the script"
        );
        // An http address keeps its path, as before.
        let script: Arc<str> = Arc::from(
            r#"
            function FindProxyForURL(url, host) {
                return url.indexOf("/proxy/") >= 0 ? "PROXY p.test:8080" : "DIRECT";
            }"#,
        );
        assert_eq!(
            routes(&script, &Url::parse("http://x.test/proxy/a.bin").unwrap()).unwrap(),
            vec![Route::Proxy("http://p.test:8080".into())]
        );
    }

    #[test]
    fn a_typical_script_is_run() {
        let script = r#"
            function FindProxyForURL(url, host) {
                if (isPlainHostName(host) || dnsDomainIs(host, ".intranet.test")) return "DIRECT";
                if (shExpMatch(url, "*/downloads/*.iso")) return "PROXY iso.proxy:3128";
                if (isInNet("10.1.2.3", "10.0.0.0", "255.0.0.0") && dnsDomainLevels(host) > 1)
                    return "PROXY proxy.corp:8080; DIRECT";
                return "DIRECT";
            }"#;
        assert_eq!(route(script, "http://intranet/a"), vec![Route::Direct]);
        assert_eq!(
            route(script, "https://wiki.intranet.test/x"),
            vec![Route::Direct]
        );
        assert_eq!(
            route(script, "http://mirror.example.org/downloads/os.iso"),
            vec![Route::Proxy("http://iso.proxy:3128".into())]
        );
        // An https address is cut to its host before the script (as browsers do), so a path
        // pattern can't route it; the host's rule applies.
        assert_eq!(
            route(script, "https://cdn.example.org/a.zip")[0],
            Route::Proxy("http://proxy.corp:8080".into())
        );
        assert_eq!(
            route(script, "https://mirror.example.org/downloads/os.iso")[0],
            Route::Proxy("http://proxy.corp:8080".into()),
            "an https path never reaches the script"
        );
    }

    #[test]
    fn helpers_behave() {
        let check = |expr: &str| {
            let script = format!(
                "function FindProxyForURL(u, h) {{ return ({expr}) ? 'DIRECT' : 'PROXY no:1'; }}"
            );
            route(&script, "http://x.test/") == vec![Route::Direct]
        };
        assert!(check("localHostOrDomainIs('www', 'www.example.org')"));
        assert!(!check(
            "localHostOrDomainIs('www.other.org', 'www.example.org')"
        ));
        assert!(check(
            "isInNet('192.168.1.20', '192.168.0.0', '255.255.0.0')"
        ));
        assert!(!check("isInNet('10.0.0.1', '192.168.0.0', '255.255.0.0')"));
        assert!(check("convert_addr('104.16.41.2') === 1745889538"));
        assert!(check(
            "shExpMatch('a.b.c', 'a.?.c') && !shExpMatch('abc', 'a?')"
        ));
        assert!(check(
            "isResolvable('localhost') && dnsResolve('localhost') === '127.0.0.1'"
        ));
        assert!(check(
            "typeof myIpAddress() === 'string' && myIpAddress().split('.').length === 4"
        ));
        assert!(check("weekdayRange('SUN', 'SAT') && timeRange(0, 23)"));
        assert!(check("dateRange('JAN', 'DEC')"));
    }

    #[test]
    fn a_broken_script_is_an_error() {
        assert!(
            routes(
                &Arc::from("this is not javascript ("),
                &Url::parse("http://x/").unwrap()
            )
            .is_err()
        );
        assert!(routes(&Arc::from("var x = 1;"), &Url::parse("http://x/").unwrap()).is_err());
    }

    #[test]
    fn a_runaway_script_is_stopped() {
        let fails = |body: &str| {
            let script = format!("function FindProxyForURL(u, h) {{ {body} }}");
            routes(&Arc::from(script), &Url::parse("http://x/").unwrap()).unwrap_err()
        };
        // Endless, even catching what stops it: stopped at the time limit.
        let started = Instant::now();
        let e = fails("for (;;) { try { for (;;) {} } catch (e) {} }");
        assert!(e.contains("interrupted"), "{e}");
        assert!(started.elapsed() < TIME_LIMIT * 3);
        // Ever more memory: stopped at the memory limit.
        let e = fails("var a = []; for (;;) a.push(new Array(1e5).fill(1.5)); return 'DIRECT';");
        assert!(e.contains("out of memory"), "{e}");
        let e = fails("var b = new ArrayBuffer(1 << 30); return 'DIRECT';");
        assert!(e.contains("out of memory"), "{e}");
        let e = fails("var s = 'x'; for (;;) s += s; return 'DIRECT';");
        assert!(e.contains("memory") || e.contains("too long"), "{e}");
        // Endless recursion: stopped at the stack limit.
        let e = fails("function f(n) { return f(n + 1) + 1; } return f(0);");
        assert!(e.contains("stack"), "{e}");
        // The script itself endless, before FindProxyForURL is called.
        assert!(
            routes(
                &Arc::from("while (true) {}"),
                &Url::parse("http://x/").unwrap()
            )
            .is_err()
        );
    }
}
