//! Embedded web dashboard for monitoring Aranet sensors.

use crate::state::AppState;
use axum::{Router, response::Html, routing::get};
use std::sync::Arc;

/// Create the dashboard router.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(dashboard_page))
        .route("/dashboard", get(dashboard_page))
}

async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Aranet Dashboard</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  :root {
    --bg-primary: #0f172a; --bg-card: #1e293b; --bg-metric: #0f172a;
    --border: #334155; --text: #e2e8f0; --text-muted: #94a3b8; --text-dim: #64748b;
    --green: #4ade80; --yellow: #facc15; --red: #f87171; --blue: #60a5fa; --purple: #a78bfa;
  }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: var(--bg-primary); color: var(--text); min-height: 100vh; }
  header { background: var(--bg-card); padding: 1rem 2rem; display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid var(--border); }
  header h1 { font-size: 1.25rem; font-weight: 600; }
  header .controls { display: flex; gap: 1rem; align-items: center; }
  header .status { font-size: 0.875rem; color: var(--text-muted); }
  header .status.live { color: var(--green); }
  header .status.live::before { content: ''; display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--green); margin-right: 6px; animation: pulse 2s infinite; }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.4; } }
  .tab-bar { background: var(--bg-card); border-bottom: 1px solid var(--border); padding: 0 2rem; display: flex; gap: 0; }
  .tab { padding: 0.75rem 1.25rem; cursor: pointer; color: var(--text-muted); border-bottom: 2px solid transparent; font-size: 0.875rem; transition: all 0.15s; }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--blue); border-bottom-color: var(--blue); }
  .container { max-width: 1400px; margin: 0 auto; padding: 2rem; }
  .tab-content { display: none; }
  .tab-content.active { display: block; }

  /* Device grid */
  .devices { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 1.5rem; }
  .card { background: var(--bg-card); border-radius: 12px; padding: 1.5rem; border: 1px solid var(--border); transition: border-color 0.2s; }
  .card:hover { border-color: var(--text-muted); }
  .card-header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1rem; }
  .card h2 { font-size: 1rem; font-weight: 600; color: #f1f5f9; }
  .card .device-id { font-size: 0.7rem; color: var(--text-dim); font-family: monospace; margin-top: 2px; }
  .card .updated { font-size: 0.7rem; color: var(--text-dim); text-align: right; }
  .metrics { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; }
  .metric { background: var(--bg-metric); padding: 0.75rem; border-radius: 8px; }
  .metric .label { font-size: 0.7rem; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .metric .value { font-size: 1.5rem; font-weight: 700; margin-top: 0.25rem; }
  .metric .unit { font-size: 0.875rem; font-weight: 400; color: var(--text-muted); }
  .metric.co2-green .value { color: var(--green); }
  .metric.co2-yellow .value { color: var(--yellow); }
  .metric.co2-red .value { color: var(--red); }
  .metric .value.normal { color: var(--blue); }
  .battery { display: flex; align-items: center; gap: 0.5rem; margin-top: 1rem; font-size: 0.875rem; color: var(--text-muted); }
  .battery .bar { width: 40px; height: 12px; background: #334155; border-radius: 3px; overflow: hidden; }
  .battery .fill { height: 100%; border-radius: 3px; transition: width 0.5s; }
  .sparkline { margin-top: 0.75rem; }
  .sparkline canvas { width: 100%; height: 40px; border-radius: 4px; }
  .empty { text-align: center; padding: 4rem; color: var(--text-dim); }
  .error { color: var(--red); text-align: center; padding: 2rem; }

  /* History tab */
  .history-controls { display: flex; gap: 1rem; align-items: center; margin-bottom: 1.5rem; flex-wrap: wrap; }
  .history-controls select, .history-controls input { background: var(--bg-card); color: var(--text); border: 1px solid var(--border); padding: 0.5rem 0.75rem; border-radius: 6px; font-size: 0.875rem; }
  .chart-container { background: var(--bg-card); border-radius: 12px; padding: 1.5rem; border: 1px solid var(--border); margin-bottom: 1.5rem; }
  .chart-container h3 { font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; }
  .chart-container canvas { width: 100%; height: 200px; }

  /* Status tab */
  .status-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 1.5rem; }
  .status-card { background: var(--bg-card); border-radius: 12px; padding: 1.5rem; border: 1px solid var(--border); }
  .status-card h3 { font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; text-transform: uppercase; letter-spacing: 0.05em; }
  .status-item { display: flex; justify-content: space-between; padding: 0.5rem 0; border-bottom: 1px solid var(--border); }
  .status-item:last-child { border-bottom: none; }
  .status-item .key { color: var(--text-muted); }
  .status-item .val { font-weight: 500; }
  .status-item .val.ok { color: var(--green); }
  .status-item .val.warn { color: var(--yellow); }
  .status-item .val.err { color: var(--red); }

  @media (max-width: 640px) { .metrics { grid-template-columns: 1fr; } .container { padding: 1rem; } .tab-bar { padding: 0 1rem; } }
</style>
</head>
<body>
<header>
  <h1>Aranet Dashboard</h1>
  <div class="controls">
    <div class="status" id="status">Connecting...</div>
  </div>
</header>
<div class="tab-bar">
  <div class="tab active" data-tab="overview">Overview</div>
  <div class="tab" data-tab="history">History</div>
  <div class="tab" data-tab="status">Service Status</div>
</div>

<!-- Overview Tab -->
<div class="container tab-content active" id="tab-overview">
  <div class="devices" id="devices">
    <div class="empty">Loading devices...</div>
  </div>
</div>

<!-- History Tab -->
<div class="container tab-content" id="tab-history">
  <div class="history-controls">
    <select id="history-device"><option value="">Select device...</option></select>
    <select id="history-metric">
      <option value="co2">CO2</option>
      <option value="temperature">Temperature</option>
      <option value="humidity">Humidity</option>
      <option value="pressure">Pressure</option>
      <option value="radon">Radon</option>
      <option value="radiation_rate">Radiation Rate</option>
      <option value="radiation_total">Radiation Total</option>
    </select>
    <select id="history-range">
      <option value="3600">1 hour</option>
      <option value="21600">6 hours</option>
      <option value="86400" selected>24 hours</option>
      <option value="604800">7 days</option>
    </select>
    <button onclick="loadHistory()" style="background:var(--blue);color:#fff;border:none;padding:0.5rem 1rem;border-radius:6px;cursor:pointer;font-size:0.875rem;">Load</button>
  </div>
  <div class="chart-container" id="history-chart-box" style="display:none;">
    <h3 id="history-chart-title">History</h3>
    <canvas id="history-canvas"></canvas>
  </div>
  <div id="history-stats" style="display:none;" class="status-grid"></div>
</div>

<!-- Status Tab -->
<div class="container tab-content" id="tab-status">
  <div class="status-grid" id="service-status">
    <div class="empty">Loading service status...</div>
  </div>
</div>

<script>
const API = window.location.origin;
const params = new URLSearchParams(window.location.search);
let apiToken = params.get('token') || window.sessionStorage.getItem('aranet_api_token') || '';
let ws;
let deviceData = {};

if (apiToken) {
  window.sessionStorage.setItem('aranet_api_token', apiToken);
}

function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function domId(value) {
  return String(value ?? '').replace(/[^a-zA-Z0-9_-]/g, '_');
}

function authHeaders() {
  return apiToken ? { 'X-API-Key': apiToken } : {};
}

async function fetchJson(path) {
  const request = () => fetch(API + path, { headers: authHeaders() });
  let res = await request();

  if (res.status === 401) {
    const entered = window.prompt(
      apiToken
        ? 'The stored API key was rejected. Enter a valid API key for this dashboard.'
        : 'API key required to access this dashboard.'
    );
    if (entered) {
      apiToken = entered.trim();
      if (apiToken) {
        window.sessionStorage.setItem('aranet_api_token', apiToken);
        res = await request();
      }
    }
  }

  if (!res.ok) {
    let message = `HTTP ${res.status}`;
    try {
      const error = await res.json();
      if (error && error.error) message = error.error;
    } catch (_) {}
    throw new Error(message);
  }

  return res.json();
}

// Tab switching
document.querySelectorAll('.tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
    document.querySelectorAll('.tab-content').forEach(tc => tc.classList.remove('active'));
    tab.classList.add('active');
    document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
    if (tab.dataset.tab === 'status') loadServiceStatus();
  });
});

function co2Class(val) {
  if (val <= 0) return '';
  if (val < 1000) return 'co2-green';
  if (val < 1400) return 'co2-yellow';
  return 'co2-red';
}

function batteryColor(pct) {
  if (pct > 50) return 'var(--green)';
  if (pct > 20) return 'var(--yellow)';
  return 'var(--red)';
}

function timeAgo(ts) {
  if (!ts) return '';
  const diff = Math.floor((Date.now() - new Date(ts).getTime()) / 1000);
  if (diff < 60) return diff + 's ago';
  if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
  if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
  return Math.floor(diff / 86400) + 'd ago';
}

function renderDevice(device) {
  const r = device.reading;
  const displayName = device.alias || device.name || device.device_id;
  const updated = timeAgo(r.captured_at);
  const cardId = `device-${domId(device.device_id)}`;
  const sparkId = `spark-${domId(device.device_id)}`;
  let metrics = '';
  if (r.co2 > 0) {
    metrics += `<div class="metric ${co2Class(r.co2)}"><div class="label">CO2</div><div class="value">${r.co2} <span class="unit">ppm</span></div></div>`;
  }
  if (r.temperature !== undefined) {
    metrics += `<div class="metric"><div class="label">Temperature</div><div class="value normal">${r.temperature.toFixed(1)} <span class="unit">\u00b0C</span></div></div>`;
  }
  if (r.humidity !== undefined && r.humidity > 0) {
    metrics += `<div class="metric"><div class="label">Humidity</div><div class="value normal">${r.humidity} <span class="unit">%</span></div></div>`;
  }
  if (r.pressure > 0) {
    metrics += `<div class="metric"><div class="label">Pressure</div><div class="value normal">${r.pressure.toFixed(1)} <span class="unit">hPa</span></div></div>`;
  }
  if (r.radon != null) {
    metrics += `<div class="metric"><div class="label">Radon</div><div class="value normal">${r.radon} <span class="unit">Bq/m\u00b3</span></div></div>`;
  }
  if (r.radiation_rate != null) {
    metrics += `<div class="metric"><div class="label">Radiation</div><div class="value normal">${r.radiation_rate.toFixed(3)} <span class="unit">\u00b5Sv/h</span></div></div>`;
  }
  const batt = r.battery || 0;

  // Mini sparkline for CO2 if we have history
  let sparklineHtml = '';
  const hist = deviceData[device.device_id]?.sparkline || [];
  if (hist.length > 1) {
    sparklineHtml = `<div class="sparkline"><canvas id="${sparkId}" width="300" height="40"></canvas></div>`;
  }

  return `<div class="card" id="${cardId}">
    <div class="card-header">
      <div><h2>${escapeHtml(displayName)}</h2>${displayName !== device.device_id ? `<div class="device-id">${escapeHtml(device.device_id)}</div>` : ''}</div>
      <div class="updated">${device.stale ? 'Stale · ' : ''}${escapeHtml(updated)}</div>
    </div>
    <div class="metrics">${metrics}</div>
    <div class="battery">
      <div class="bar"><div class="fill" style="width:${batt}%;background:${batteryColor(batt)}"></div></div>
      ${batt}%
    </div>
    ${sparklineHtml}
  </div>`;
}

function drawSparkline(canvasId, data, thresholds) {
  const canvas = document.getElementById(canvasId);
  if (!canvas || !data.length) return;
  const ctx = canvas.getContext('2d');
  const w = canvas.width = canvas.offsetWidth * 2;
  const h = canvas.height = 80;
  ctx.clearRect(0, 0, w, h);

  const min = Math.min(...data) * 0.95;
  const max = Math.max(...data) * 1.05 || 1;
  const range = max - min || 1;

  // Draw threshold lines
  if (thresholds) {
    for (const t of thresholds) {
      const y = h - ((t.value - min) / range) * h;
      if (y > 0 && y < h) {
        ctx.strokeStyle = t.color + '40';
        ctx.lineWidth = 1;
        ctx.setLineDash([4, 4]);
        ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(w, y); ctx.stroke();
        ctx.setLineDash([]);
      }
    }
  }

  // Draw line
  ctx.strokeStyle = '#60a5fa';
  ctx.lineWidth = 2;
  ctx.lineJoin = 'round';
  ctx.beginPath();
  for (let i = 0; i < data.length; i++) {
    const x = (i / (data.length - 1)) * w;
    const y = h - ((data[i] - min) / range) * h;
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  }
  ctx.stroke();

  // Fill gradient
  const grad = ctx.createLinearGradient(0, 0, 0, h);
  grad.addColorStop(0, '#60a5fa20');
  grad.addColorStop(1, '#60a5fa00');
  ctx.lineTo(w, h); ctx.lineTo(0, h); ctx.closePath();
  ctx.fillStyle = grad; ctx.fill();
}

async function fetchDevices() {
  try {
    const [devices, configResult, knownDevices] = await Promise.all([
      fetchJson('/api/devices/current'),
      fetchJson('/api/config').catch(() => ({ devices: [] })),
      fetchJson('/api/devices').catch(() => []),
    ]);
    const el = document.getElementById('devices');
    const select = document.getElementById('history-device');
    const historyCatalog = buildHistoryDeviceCatalog(configResult, knownDevices, devices);

    if (!Array.isArray(devices) || devices.length === 0) {
      el.innerHTML = '<div class="empty">No readings yet. Configure devices in server.toml.</div>';
      updateHistoryDeviceSelector(select, historyCatalog);
      return false;
    }

    // Update device data and sparklines
    for (const device of devices) {
      if (!deviceData[device.device_id]) deviceData[device.device_id] = { sparkline: [] };
      deviceData[device.device_id].alias = device.alias;
      deviceData[device.device_id].name = device.name;
      const spark = deviceData[device.device_id].sparkline;
      const val = device.reading.co2 > 0 ? device.reading.co2 : device.reading.temperature;
      spark.push(val);
      if (spark.length > 60) spark.shift();
    }

    el.innerHTML = devices.map(renderDevice).join('');

    // Draw sparklines
    for (const device of devices) {
      const hist = deviceData[device.device_id]?.sparkline || [];
      if (hist.length > 1) {
        drawSparkline(`spark-${domId(device.device_id)}`, hist,
          device.reading.co2 > 0 ? [{value: 1000, color: '#facc15'}, {value: 1400, color: '#f87171'}] : null);
      }
    }

    // Populate device selector
    updateHistoryDeviceSelector(select, historyCatalog);
    return true;
  } catch(e) {
    document.getElementById('devices').innerHTML = `<div class="error">Failed to load: ${escapeHtml(e.message)}</div>`;
    return false;
  }
}

function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = apiToken ? `?token=${encodeURIComponent(apiToken)}` : '';
  ws = new WebSocket(`${proto}//${location.host}/api/ws${token}`);
  ws.onopen = () => {
    const el = document.getElementById('status');
    el.textContent = 'Live';
    el.className = 'status live';
  };
  ws.onmessage = (e) => {
    try {
      const d = JSON.parse(e.data);
      if (d.type === 'error' || (d.error && !d.reading)) {
        const el = document.getElementById('status');
        el.textContent = d.error || 'Snapshot error';
        el.className = 'status';
        return;
      }
      // Update sparkline data
      if (!deviceData[d.device_id]) deviceData[d.device_id] = { sparkline: [] };
      const spark = deviceData[d.device_id].sparkline;
      const val = d.reading.co2 > 0 ? d.reading.co2 : d.reading.temperature;
      spark.push(val);
      if (spark.length > 60) spark.shift();

      const device = {
        device_id: d.device_id,
        alias: deviceData[d.device_id].alias || null,
        name: deviceData[d.device_id].name || null,
        age_seconds: 0,
        stale: false,
        reading: d.reading
      };

      const card = document.getElementById(`device-${domId(d.device_id)}`);
      if (card) {
        card.outerHTML = renderDevice(device);
        // Redraw sparkline
        const hist = deviceData[d.device_id]?.sparkline || [];
        if (hist.length > 1) {
          drawSparkline(`spark-${domId(d.device_id)}`, hist,
            d.reading.co2 > 0 ? [{value: 1000, color: '#facc15'}, {value: 1400, color: '#f87171'}] : null);
        }
      } else {
        fetchDevices();
      }
    } catch(_) {}
  };
  ws.onclose = () => {
    const el = document.getElementById('status');
    el.textContent = 'Reconnecting...';
    el.className = 'status';
    setTimeout(connectWs, 3000);
  };
}

// History chart
async function loadHistory() {
  const device = document.getElementById('history-device').value;
  const metric = document.getElementById('history-metric').value;
  const range = parseInt(document.getElementById('history-range').value);
  if (!device) return;

  const since = Math.floor(Date.now() / 1000) - range;
  try {
    const result = await fetchJson(`/api/devices/${encodeURIComponent(device)}/history?since=${since}&limit=1000`);
    const readings = Array.isArray(result.data) ? result.data : [];
    if (!readings.length) {
      document.getElementById('history-chart-box').style.display = 'none';
      document.getElementById('history-stats').style.display = 'grid';
      document.getElementById('history-stats').innerHTML = `
        <div class="status-card"><h3>No History</h3>
          <div class="status-item"><span class="key">Status</span><span class="val">No cached history found for this device and time range</span></div>
        </div>`;
      return;
    }

    const values = readings
      .map(r => {
        const timestamp = r.timestamp || r.captured_at || r.synced_at;
        return { t: timestamp ? new Date(timestamp) : null, v: r[metric] };
      })
      .filter(d => d.v != null && d.t && !Number.isNaN(d.t.getTime()));
    if (!values.length) {
      document.getElementById('history-chart-box').style.display = 'none';
      document.getElementById('history-stats').style.display = 'grid';
      document.getElementById('history-stats').innerHTML = `
        <div class="status-card"><h3>Metric Unavailable</h3>
          <div class="status-item"><span class="key">Status</span><span class="val">The selected metric is not present in the cached history for this device</span></div>
        </div>`;
      return;
    }

    document.getElementById('history-chart-box').style.display = 'block';
    const units = {
      co2: 'ppm',
      temperature: '\u00b0C',
      humidity: '%',
      pressure: 'hPa',
      radon: 'Bq/m\u00b3',
      radiation_rate: '\u00b5Sv/h',
      radiation_total: 'mSv',
    };
    document.getElementById('history-chart-title').textContent = `${metric.charAt(0).toUpperCase() + metric.slice(1)} - ${device} (${units[metric] || ''})`;

    drawHistoryChart(values, metric);

    // Stats
    const vals = values.map(d => d.v);
    const min = Math.min(...vals).toFixed(1);
    const max = Math.max(...vals).toFixed(1);
    const avg = (vals.reduce((a, b) => a + b, 0) / vals.length).toFixed(1);
    document.getElementById('history-stats').style.display = 'grid';
    document.getElementById('history-stats').innerHTML = `
      <div class="status-card"><h3>Statistics</h3>
        <div class="status-item"><span class="key">Min</span><span class="val">${min} ${units[metric] || ''}</span></div>
        <div class="status-item"><span class="key">Max</span><span class="val">${max} ${units[metric] || ''}</span></div>
        <div class="status-item"><span class="key">Average</span><span class="val">${avg} ${units[metric] || ''}</span></div>
        <div class="status-item"><span class="key">Samples</span><span class="val">${values.length}</span></div>
      </div>`;
  } catch(e) {
    document.getElementById('history-chart-box').style.display = 'none';
    document.getElementById('history-stats').style.display = 'grid';
    document.getElementById('history-stats').innerHTML = `<div class="error">Failed to load history: ${escapeHtml(e.message)}</div>`;
  }
}

function drawHistoryChart(values, metric) {
  const canvas = document.getElementById('history-canvas');
  const ctx = canvas.getContext('2d');
  const w = canvas.width = canvas.offsetWidth * 2;
  const h = canvas.height = 400;
  ctx.clearRect(0, 0, w, h);

  const pad = { top: 20, right: 20, bottom: 40, left: 60 };
  const cw = w - pad.left - pad.right;
  const ch = h - pad.top - pad.bottom;

  const vals = values.map(d => d.v);
  const times = values.map(d => d.t.getTime());
  const minV = Math.min(...vals) * 0.95;
  const maxV = Math.max(...vals) * 1.05 || 1;
  const minT = Math.min(...times);
  const maxT = Math.max(...times);
  const rangeV = maxV - minV || 1;
  const rangeT = maxT - minT || 1;

  const toX = t => pad.left + ((t - minT) / rangeT) * cw;
  const toY = v => pad.top + ch - ((v - minV) / rangeV) * ch;

  // Grid lines
  ctx.strokeStyle = '#334155';
  ctx.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const y = pad.top + (ch / 4) * i;
    ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(w - pad.right, y); ctx.stroke();
    const label = (maxV - (rangeV / 4) * i).toFixed(0);
    ctx.fillStyle = '#94a3b8'; ctx.font = '20px sans-serif'; ctx.textAlign = 'right';
    ctx.fillText(label, pad.left - 8, y + 6);
  }

  // Time labels
  const steps = Math.min(6, values.length);
  for (let i = 0; i <= steps; i++) {
    const t = minT + (rangeT / steps) * i;
    const x = toX(t);
    const d = new Date(t);
    const label = d.getHours().toString().padStart(2, '0') + ':' + d.getMinutes().toString().padStart(2, '0');
    ctx.fillStyle = '#94a3b8'; ctx.font = '20px sans-serif'; ctx.textAlign = 'center';
    ctx.fillText(label, x, h - 10);
  }

  // Threshold lines for CO2
  if (metric === 'co2') {
    for (const [threshold, color] of [[1000, '#facc15'], [1400, '#f87171']]) {
      const y = toY(threshold);
      if (y > pad.top && y < pad.top + ch) {
        ctx.strokeStyle = color + '60'; ctx.lineWidth = 2;
        ctx.setLineDash([6, 4]); ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(w - pad.right, y); ctx.stroke(); ctx.setLineDash([]);
        ctx.fillStyle = color; ctx.font = '18px sans-serif'; ctx.textAlign = 'left';
        ctx.fillText(threshold + ' ppm', w - pad.right + 4, y + 5);
      }
    }
  }

  // Data line
  ctx.strokeStyle = '#60a5fa'; ctx.lineWidth = 3; ctx.lineJoin = 'round';
  ctx.beginPath();
  for (let i = 0; i < values.length; i++) {
    const x = toX(values[i].t.getTime());
    const y = toY(values[i].v);
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  }
  ctx.stroke();

  // Fill
  const grad = ctx.createLinearGradient(0, pad.top, 0, pad.top + ch);
  grad.addColorStop(0, '#60a5fa18');
  grad.addColorStop(1, '#60a5fa00');
  ctx.lineTo(toX(times[times.length - 1]), pad.top + ch);
  ctx.lineTo(toX(times[0]), pad.top + ch);
  ctx.closePath(); ctx.fillStyle = grad; ctx.fill();
}

// Service status
async function loadServiceStatus() {
  try {
    const [status, health] = await Promise.all([
      fetchJson('/api/status'),
      fetchJson('/api/health/detailed')
    ]);

    let html = '';
    const serviceStatusClass = health.status === 'ok' ? 'ok' : health.status === 'degraded' ? 'warn' : 'err';

    // Service info
    html += `<div class="status-card"><h3>Service</h3>
      <div class="status-item"><span class="key">Version</span><span class="val">${escapeHtml(health.version || 'unknown')}</span></div>
      <div class="status-item"><span class="key">Status</span><span class="val ${serviceStatusClass}">${escapeHtml(health.status || 'unknown')}</span></div>
      <div class="status-item"><span class="key">Collector</span><span class="val ${status.collector && status.collector.running ? 'ok' : 'warn'}">${status.collector && status.collector.running ? 'Running' : 'Stopped'}</span></div>
      ${status.collector && status.collector.uptime_seconds ? `<div class="status-item"><span class="key">Uptime</span><span class="val">${formatDuration(status.collector.uptime_seconds)}</span></div>` : ''}
      <div class="status-item"><span class="key">Devices</span><span class="val">${Array.isArray(status.devices) ? status.devices.length : 0}</span></div>
    </div>`;

    // Device stats
    if (status.devices && status.devices.length) {
      for (const dev of status.devices) {
        const successRate = dev.success_count + dev.failure_count > 0
          ? ((dev.success_count / (dev.success_count + dev.failure_count)) * 100).toFixed(1)
          : 'N/A';
        html += `<div class="status-card"><h3>${escapeHtml(dev.alias || dev.device_id)}</h3>
          <div class="status-item"><span class="key">Address</span><span class="val" style="font-family:monospace;font-size:0.8rem">${escapeHtml(dev.device_id)}</span></div>
          <div class="status-item"><span class="key">Poll Interval</span><span class="val">${dev.poll_interval}s</span></div>
          <div class="status-item"><span class="key">Success Rate</span><span class="val ${successRate === 'N/A' ? '' : parseFloat(successRate) > 90 ? 'ok' : parseFloat(successRate) > 50 ? 'warn' : 'err'}">${successRate}%</span></div>
          <div class="status-item"><span class="key">Successes</span><span class="val">${dev.success_count}</span></div>
          <div class="status-item"><span class="key">Failures</span><span class="val ${dev.failure_count > 0 ? 'warn' : ''}">${dev.failure_count}</span></div>
          ${dev.last_poll_duration_ms != null ? `<div class="status-item"><span class="key">Last Poll</span><span class="val">${dev.last_poll_duration_ms}ms</span></div>` : ''}
          ${dev.last_error ? `<div class="status-item"><span class="key">Last Error</span><span class="val err" style="font-size:0.8rem;max-width:200px;word-break:break-word">${escapeHtml(dev.last_error)}</span></div>` : ''}
        </div>`;
      }
    } else {
      html += '<div class="status-card"><h3>Devices</h3><div class="status-item"><span class="key">Status</span><span class="val">No collector statistics yet</span></div></div>';
    }

    document.getElementById('service-status').innerHTML = html;
  } catch(e) {
    document.getElementById('service-status').innerHTML = `<div class="error">Failed to load status: ${escapeHtml(e.message)}</div>`;
  }
}

function formatDuration(secs) {
  if (secs < 60) return secs + 's';
  if (secs < 3600) return Math.floor(secs / 60) + 'm ' + (secs % 60) + 's';
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h < 24) return h + 'h ' + m + 'm';
  return Math.floor(h / 24) + 'd ' + (h % 24) + 'h';
}

function buildHistoryDeviceCatalog(configResult, knownDevices, currentDevices) {
  const catalog = new Map();
  const upsert = (deviceId, patch) => {
    if (!deviceId) return;
    const existing = catalog.get(deviceId) || {
      device_id: deviceId,
      alias: null,
      name: null,
      configured: false,
      known: false,
      current: false,
    };
    catalog.set(deviceId, { ...existing, ...patch });
  };

  const configured = Array.isArray(configResult && configResult.devices) ? configResult.devices : [];
  for (const device of configured) {
    upsert(device.address, {
      alias: device.alias || null,
      configured: true,
    });
  }

  const known = Array.isArray(knownDevices) ? knownDevices : [];
  for (const device of known) {
    upsert(device.id, {
      name: device.name || null,
      known: true,
    });
  }

  const current = Array.isArray(currentDevices) ? currentDevices : [];
  for (const device of current) {
    upsert(device.device_id, {
      alias: device.alias || null,
      name: device.name || null,
      current: true,
    });
  }

  return Array.from(catalog.values()).sort((a, b) => {
    const aLabel = (a.alias || a.name || a.device_id).toLowerCase();
    const bLabel = (b.alias || b.name || b.device_id).toLowerCase();
    return aLabel.localeCompare(bLabel);
  });
}

function updateHistoryDeviceSelector(select, devices) {
  if (!select) return;
  const currentVal = select.value;
  if (!Array.isArray(devices) || devices.length === 0) {
    select.innerHTML = '<option value="">Select device...</option>';
    return;
  }

  const opts = devices.map(device => {
    const label = device.alias || device.name || device.device_id;
    return `<option value="${escapeHtml(device.device_id)}">${escapeHtml(label)}</option>`;
  });
  select.innerHTML = '<option value="">Select device...</option>' + opts.join('');
  if (currentVal) select.value = currentVal;
}

async function initDashboard() {
  await fetchDevices();
  connectWs();
  setInterval(fetchDevices, 60000);
}

initDashboard();
</script>
</body>
</html>"##;
