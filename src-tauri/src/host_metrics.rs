use crate::sessions::SessionManager;
use crate::ssh_remote::exec_command;
use base64::Engine;
const STATS_SCRIPT: &str = r###"
set -e
read -r _ u1 n1 s1 i1 iw1 irq1 si1 st1 _ < /proc/stat
idle1=$((i1+iw1)); total1=$((u1+n1+s1+idle1+irq1+si1+st1))
sleep 1
read -r _ u2 n2 s2 i2 iw2 irq2 si2 st2 _ < /proc/stat
idle2=$((i2+iw2)); total2=$((u2+n2+s2+idle2+irq2+si2+st2))
dt=$((total2-total1)); di=$((idle2-idle1))
cpu=0
if [ "$dt" -gt 0 ]; then cpu=$(awk -v d="$dt" -v i="$di" 'BEGIN{printf "%.1f",(d-i)*100/d}'); fi

hostname=$(hostname -s 2>/dev/null || hostname 2>/dev/null || echo unknown)
cores=$(nproc 2>/dev/null || echo 1)
kernel=$(uname -r 2>/dev/null || echo unknown)
uptime_sec=$(awk '{print int($1)}' /proc/uptime)
read -r load1 load5 load15 _ < /proc/loadavg

mem_total_kb=$(awk '/^MemTotal:/{print $2}' /proc/meminfo)
mem_avail_kb=$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)
[ -z "$mem_avail_kb" ] && mem_avail_kb=$(awk '/^MemFree:/{print $2}' /proc/meminfo)
mem_used_kb=$((mem_total_kb - mem_avail_kb))
mem_pct=0
if [ "$mem_total_kb" -gt 0 ]; then mem_pct=$(awk -v t="$mem_total_kb" -v u="$mem_used_kb" 'BEGIN{printf "%.1f",u*100/t}'); fi

swap_total_kb=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo)
swap_free_kb=$(awk '/^SwapFree:/{print $2}' /proc/meminfo)
swap_used_kb=$((swap_total_kb - swap_free_kb))

os_name=unknown
if [ -f /etc/os-release ]; then
  os_name=$(awk -F= '/^PRETTY_NAME=/{gsub(/"/,""); print $2; exit}' /etc/os-release)
fi
[ "$os_name" = unknown ] && os_name=$(uname -sr 2>/dev/null || echo unknown)

echo "@HOST|$hostname"
echo "@OS|$os_name"
echo "@KERNEL|$kernel"
echo "@CORES|$cores"
echo "@UPTIME|$uptime_sec"
echo "@CPU|$cpu"
echo "@MEM|$mem_total_kb|$mem_used_kb|$mem_pct"
echo "@SWAP|$swap_total_kb|$swap_used_kb"
echo "@LOAD|$load1|$load5|$load15"
df -B1 -P -x tmpfs -x devtmpfs -x squashfs -x overlay 2>/dev/null | awk 'NR>1{gsub(/%/,"",$5); print "@DISK|"$6"|"$2"|"$3"|"$4"|"$5}' || true
"###;

const PM2_SCRIPT: &str = r###"
node <<'NODE'
const { execSync } = require('child_process')

function findPm2() {
  const tries = [
    'command -v pm2 2>/dev/null',
    'test -x /usr/local/bin/pm2 && echo /usr/local/bin/pm2',
    'test -x /usr/bin/pm2 && echo /usr/bin/pm2'
  ]
  for (const cmd of tries) {
    try {
      const hit = execSync(cmd, { encoding: 'utf8', shell: '/bin/bash' }).trim().split(/\r?\n/)[0]
      if (hit) return hit
    } catch {}
  }
  return ''
}

const bin = findPm2()
if (!bin) {
  console.log('@PM2_ERR|not_installed')
  process.exit(0)
}

let raw = ''
try {
  raw = execSync('"' + bin.replace(/"/g, '') + '" jlist', {
    encoding: 'utf8',
    maxBuffer: 20 * 1024 * 1024,
    timeout: 12000
  })
} catch (e) {
  console.log('@PM2_ERR|failed')
  process.exit(0)
}

let list
try {
  list = JSON.parse(raw)
} catch {
  console.log('@PM2_ERR|parse')
  process.exit(0)
}

if (!Array.isArray(list)) {
  console.log('@PM2_ERR|parse')
  process.exit(0)
}

const now = Date.now()
for (const p of list) {
  const env = p.pm2_env || {}
  const mon = p.monit || {}
  const status = String(env.status || 'unknown')
  const pmUptime = Number(env.pm_uptime || 0)
  let uptimeSec = 0
  if (pmUptime > 0 && (status === 'online' || status === 'launching')) {
    uptimeSec = Math.max(0, Math.floor((now - pmUptime) / 1000))
  }
  const row = {
    id: Number(p.pm_id ?? env.pm_id ?? 0),
    name: String(env.name || p.name || 'unknown'),
    status,
    uptimeSec,
    cpu: Number(mon.cpu || 0),
    mem: Number(mon.memory || 0),
    pid: Number(p.pid || 0),
    restarts: Number(env.restart_time || 0)
  }
  console.log('@PROC|' + Buffer.from(JSON.stringify(row)).toString('base64'))
}
NODE
"###;

pub async fn fetch_host_stats(
    mgr: &SessionManager,
    connection_id: &str,
) -> Result<serde_json::Value, String> {
    let res = exec_command(mgr, connection_id, STATS_SCRIPT, 15_000).await?;
    if res.code != 0 && res.stdout.trim().is_empty() {
        let msg = if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        };
        return Err(if msg.is_empty() {
            "无法读取主机状态".into()
        } else {
            msg
        });
    }
    let stats = parse_stats(&res.stdout)?;
    Ok(stats)
}

fn parse_stats(stdout: &str) -> Result<serde_json::Value, String> {
    let mut kv: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut disks: Vec<serde_json::Value> = Vec::new();

    for line in stdout.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("@DISK|") {
            let parts: Vec<&str> = rest.split('|').collect();
            if parts.len() >= 5 {
                disks.push(serde_json::json!({
                    "mount": parts[0],
                    "sizeBytes": parts[1].parse::<u64>().unwrap_or(0),
                    "usedBytes": parts[2].parse::<u64>().unwrap_or(0),
                    "availBytes": parts[3].parse::<u64>().unwrap_or(0),
                    "usePercent": parts[4].parse::<f64>().unwrap_or(0.0),
                }));
            }
            continue;
        }
        if line.starts_with('@') {
            if let Some(idx) = line.find('|') {
                let key = line[1..idx].to_string();
                kv.insert(key, line[idx + 1..].to_string());
            }
        }
    }

    let mem_parts: Vec<&str> = kv.get("MEM").map(|s| s.split('|').collect()).unwrap_or_default();
    let swap_parts: Vec<&str> = kv.get("SWAP").map(|s| s.split('|').collect()).unwrap_or_default();
    let load_parts: Vec<&str> = kv.get("LOAD").map(|s| s.split('|').collect()).unwrap_or_default();

    let mem_total_kb = mem_parts.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let mem_used_kb = mem_parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let mem_pct = mem_parts.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let swap_total_kb = swap_parts.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let swap_used_kb = swap_parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    let hostname = kv.get("HOST").cloned().unwrap_or_else(|| "—".into());
    let collected_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    if hostname == "—" && disks.is_empty() && mem_total_kb == 0 {
        return Err("状态数据解析失败".into());
    }

    Ok(serde_json::json!({
        "hostname": hostname,
        "os": kv.get("OS").cloned().unwrap_or_else(|| "—".into()),
        "kernel": kv.get("KERNEL").cloned().unwrap_or_else(|| "—".into()),
        "cores": kv.get("CORES").and_then(|s| s.parse().ok()).unwrap_or(1),
        "uptimeSec": kv.get("UPTIME").and_then(|s| s.parse().ok()).unwrap_or(0),
        "cpuPercent": kv.get("CPU").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        "memTotalBytes": mem_total_kb * 1024,
        "memUsedBytes": mem_used_kb * 1024,
        "memPercent": mem_pct,
        "swapTotalBytes": swap_total_kb * 1024,
        "swapUsedBytes": swap_used_kb * 1024,
        "load1": load_parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0),
        "load5": load_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        "load15": load_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        "disks": disks,
        "collectedAt": collected_at
    }))
}

pub async fn fetch_host_pm2(
    mgr: &SessionManager,
    connection_id: &str,
) -> Result<serde_json::Value, String> {
    let res = exec_command(mgr, connection_id, PM2_SCRIPT, 20_000).await?;
    if res.code != 0 && res.stdout.trim().is_empty() {
        let msg = if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        };
        return Err(if msg.is_empty() {
            "无法读取 pm2 状态".into()
        } else {
            msg
        });
    }
    Ok(parse_pm2(&res.stdout, &res.stderr))
}

fn parse_pm2(stdout: &str, stderr: &str) -> serde_json::Value {
    let mut processes: Vec<serde_json::Value> = Vec::new();
    let mut notice: Option<String> = None;

    for line in stdout.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if let Some(code) = line.strip_prefix("@PM2_ERR|") {
            notice = Some(match code {
                "not_installed" => "未安装 pm2".into(),
                "parse" => "pm2 数据解析失败".into(),
                _ => "无法读取 pm2 进程列表".into(),
            });
            continue;
        }
        if let Some(b64) = line.strip_prefix("@PROC|") {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    processes.push(serde_json::json!({
                        "pmId": row.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                        "name": row.get("name").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "status": row.get("status").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "uptimeSec": row.get("uptimeSec").and_then(|v| v.as_u64()).unwrap_or(0),
                        "cpuPercent": row.get("cpu").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        "memoryBytes": row.get("mem").and_then(|v| v.as_u64()).unwrap_or(0),
                        "pid": row.get("pid").and_then(|v| v.as_u64()).unwrap_or(0),
                        "restarts": row.get("restarts").and_then(|v| v.as_u64()).unwrap_or(0),
                    }));
                }
            }
        }
    }

    processes.sort_by(|a, b| {
        let ma = a.get("memoryBytes").and_then(|v| v.as_u64()).unwrap_or(0);
        let mb = b.get("memoryBytes").and_then(|v| v.as_u64()).unwrap_or(0);
        ma.cmp(&mb).reverse()
    });

    let collected_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let final_notice = if processes.is_empty() {
        notice.or_else(|| Some("暂无 pm2 进程".into()))
    } else {
        notice
    };

    if processes.is_empty() && final_notice.is_none() && !stderr.trim().is_empty() {
        return serde_json::json!({
            "processes": [],
            "collectedAt": collected_at,
            "notice": stderr.trim()
        });
    }

    serde_json::json!({
        "processes": processes,
        "collectedAt": collected_at,
        "notice": final_notice
    })
}
