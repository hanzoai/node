//! hanzo.exchange compute DEX — a live HTTP face for the HVM (HIP-0008).
//! std-only (no deps). Pools are GPU classes; prices are conjugate momenta
//! discovered by the symplectic Hamiltonian market. Run: `cargo run --bin dex`.
use hanzo_hvm::Market;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

const NAMES: [&str; 4] = ["H100-hour", "RTX4090-hour", "M4Max-hour", "CPU-hour"];

fn market() -> Market {
    Market {
        q: vec![1000.0, 2000.0, 5000.0, 100000.0],
        p: vec![3.50, 0.80, 0.30, 0.02], // starting $AI/hour
        mass: vec![80.0, 50.0, 40.0, 200.0],
        scarcity: vec![300.0, 80.0, 30.0, 2.0],
    }
}

fn pools_json(m: &Market) -> String {
    let rows: Vec<String> = (0..NAMES.len())
        .map(|i| format!(r#"{{"id":{i},"resource":"{}","price_ai_per_hr":{:.4},"available":{:.0}}}"#, NAMES[i], m.p[i], m.q[i]))
        .collect();
    format!(r#"{{"pools":[{}],"hamiltonian":{:.4},"compute_invariant":{:.2}}}"#, rows.join(","), m.hamiltonian(), m.compute_invariant())
}

fn qparam(query: &str, key: &str) -> Option<f64> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key { v.parse().ok() } else { None }
    })
}

fn main() {
    let m = Mutex::new(market());
    let l = TcpListener::bind("127.0.0.1:9700").expect("bind :9700");
    println!("HVM compute DEX (hanzo.exchange backend) live on http://127.0.0.1:9700");
    for s in l.incoming() {
        let mut s = match s { Ok(s) => s, Err(_) => continue };
        let mut buf = [0u8; 2048];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let line = req.lines().next().unwrap_or("");
        let path = line.split_whitespace().nth(1).unwrap_or("/");
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let body = {
            let mut m = m.lock().unwrap();
            match route {
                "/pools" => pools_json(&m),
                "/quote" => {
                    let r = qparam(query, "r").unwrap_or(0.0) as usize;
                    let amt = qparam(query, "amt").unwrap_or(1.0);
                    let mut preview = m.clone();
                    let price = preview.quote(r.min(NAMES.len()-1), amt, 0.1, 1.0);
                    format!(r#"{{"resource":"{}","amount":{:.0},"clearing_price_ai_per_hr":{:.4}}}"#, NAMES[r.min(NAMES.len()-1)], amt, price)
                }
                "/buy" => {
                    let r = (qparam(query, "r").unwrap_or(0.0) as usize).min(NAMES.len()-1);
                    let amt = qparam(query, "amt").unwrap_or(1.0);
                    let price = m.quote(r, amt, 0.1, 1.0);
                    format!(r#"{{"filled":"{}","amount":{:.0},"price_ai_per_hr":{:.4},"remaining":{:.0}}}"#, NAMES[r], amt, price, m.q[r])
                }
                _ => r#"{"endpoints":["/pools","/quote?r=0&amt=100","/buy?r=0&amt=100"]}"#.to_string(),
            }
        };
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
    }
}
