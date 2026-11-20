//! The dump line and glance use the same measurements as the structured row.
use crate::threads::percent;
use serde_json::{Value, json};

pub(crate) fn describe(group: &str, r: &mut Value, name: &str, waiters: Option<&[String]>) {
    let w = match waiters {
        None => "unknown".into(),
        Some([]) => "-".into(),
        Some(ws) => ws.join(","),
    };
    let waiting = waiters.is_some_and(|ws| !ws.is_empty());
    let level = |blocked, busy| {
        if blocked {
            "blocked"
        } else if busy {
            "busy"
        } else {
            "quiet"
        }
    };
    let n = |key: &str| r[key].as_u64().expect("required numeric object field");
    let pct = |a: u64, b: u64| percent(a.into(), b.into());
    let (line, fill, text, level) = match group {
        "heaps" => {
            let (f, a, m) = (
                n("free_bytes"),
                n("allocated_bytes"),
                n("max_allocated_bytes"),
            );
            (
                format!("heap {name:20} free={f} alloc={a} max={m} waiters={w}"),
                pct(a, a + f),
                format!("{a}/{}B", a + f),
                level(a > 0 && f == 0, waiting),
            )
        }
        "semaphores" => {
            let (c, l) = (n("count"), n("limit"));
            let (line, text, fill) = match l {
                0 => ("not initialized".into(), "NOT INITIALIZED".into(), 0.0),
                0xffffffff => (format!("{c}/-"), format!("{c} / unlimited"), 0.0),
                _ => (format!("{c}/{l}"), format!("{c}/{l}"), pct(c, l)),
            };
            (
                format!("sem  {name:20} {line}  waiters={w}"),
                fill,
                text,
                level(false, waiting),
            )
        }
        "mutexes" => {
            let (a, c) = (n("owner_address"), n("lock_count"));
            let locked = a != 0;
            let status = if locked { "LOCKED" } else { "FREE" };
            let owner = r["owner_name"].as_str();
            let line = format!(
                "mtx  {name:20} {status:6} owner={} depth={c} waiters={w}",
                owner
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("0x{a:x}"))
            );
            let mut text = status.to_owned();
            if locked {
                text += &format!(
                    " ● {}",
                    owner
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("0x{a:X}"))
                );
                if c > 1 {
                    text += &format!(" ×{c}")
                }
            }
            (
                line,
                if locked { 100.0 } else { 0.0 },
                text,
                level(locked && waiting, locked),
            )
        }
        "msgqs" | "mem_slabs" => {
            let slab = group == "mem_slabs";
            let (tag, u, m, s) = if slab {
                ("slab", n("num_used"), n("num_blocks"), n("block_size"))
            } else {
                ("msgq", n("used_msgs"), n("max_msgs"), n("msg_size"))
            };
            let peak = if slab {
                format!(
                    " peak={}",
                    r["max_used"]
                        .as_u64()
                        .map(|x| x.to_string())
                        .as_deref()
                        .unwrap_or("-")
                )
            } else {
                String::new()
            };
            (
                format!("{tag} {name:20} {u}/{m} x {s}B{peak} waiters={w}"),
                pct(u, m),
                format!("{u}/{m} × {s}B"),
                level(m != 0 && u >= m, waiting),
            )
        }
        "workqs" => {
            let f = n("flags");
            let busy = f & 2 != 0;
            let flags = ["started", "busy", "draining", "plugged"]
                .into_iter()
                .enumerate()
                .filter(|(i, _)| f & (1 << i) != 0)
                .map(|(_, s)| s)
                .collect::<Vec<_>>()
                .join(",");
            let pending = r["pending"].as_array().unwrap();
            let depth = pending.len();
            let truncated = r["is_pending_truncated"] == true;
            let mut items = pending
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(",");
            if truncated {
                items += ",...";
            }
            let mut text = if busy {
                "BUSY"
            } else if f & 1 != 0 {
                "IDLE"
            } else {
                "NOT STARTED"
            }
            .to_owned();
            if depth > 0 {
                text += &format!(" ● {depth}{} pending", if truncated { "+" } else { "" })
            }
            (
                format!(
                    "wkq  {name:20} {depth} pending flags={} thread={} items={} waiters={w}",
                    if flags.is_empty() { "-" } else { &flags },
                    r["thread_name"].as_str().unwrap_or("-"),
                    if items.is_empty() { "-" } else { &items }
                ),
                if busy { 100.0 } else { 0.0 },
                text,
                level(f & 12 != 0, depth > 0 || waiting),
            )
        }
        _ => unreachable!(),
    };
    if group == "heaps" {
        r["usage_percent"] = json!(fill);
    }
    r["line"] = json!(line);
    r["glance"] = json!({"fill":fill,"text":text,"level":level});
}
