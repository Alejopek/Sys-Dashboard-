const socket = io();

/* ── Reloj en vivo ────────────────────────────── */
function updateClock() {
  const el = document.getElementById("live-time");
  if (el) el.textContent = new Date().toLocaleTimeString("es-AR", { hour12: false });
}
updateClock();
setInterval(updateClock, 1000);

/* ── Chart: CPU en tiempo real ───────────────── */
const ctx = document.getElementById("cpuChart").getContext("2d");
const cpuChart = new Chart(ctx, {
  type: "line",
  data: {
    labels: [],
    datasets: [
      {
        label: "CPU %",
        data: [],
        borderColor: "#00c896",
        backgroundColor: "rgba(0, 200, 150, 0.07)",
        borderWidth: 1.5,
        tension: 0.45,
        fill: true,
        pointRadius: 0,
        pointHitRadius: 10,
      },
    ],
  },
  options: {
    responsive: true,
    maintainAspectRatio: false,
    animation: { duration: 300 },
    scales: {
      y: {
        beginAtZero: true,
        max: 100,
        grid: { color: "rgba(255,255,255,0.04)" },
        ticks: { color: "#4a4f62", font: { family: "JetBrains Mono", size: 10 }, stepSize: 25 },
        border: { display: false },
      },
      x: {
        grid: { display: false },
        ticks: { color: "#4a4f62", font: { family: "JetBrains Mono", size: 10 }, maxTicksLimit: 6 },
        border: { display: false },
      },
    },
    plugins: {
      legend: { display: false },
      tooltip: {
        backgroundColor: "#16191f",
        borderColor: "rgba(255,255,255,0.08)",
        borderWidth: 1,
        titleColor: "#7c8296",
        bodyColor: "#00c896",
        bodyFont: { family: "JetBrains Mono" },
        padding: 10,
      },
    },
  },
});

/* ── Chart: Historial ────────────────────────── */
const ctxHistory = document.getElementById("historyChart").getContext("2d");
const historyChart = new Chart(ctxHistory, {
  type: "line",
  data: {
    labels: [],
    datasets: [
      {
        label: "CPU %",
        data: [],
        borderColor: "#3b9eff",
        backgroundColor: "rgba(59,158,255,0.06)",
        borderWidth: 1.5,
        tension: 0.3,
        fill: true,
        pointRadius: 0,
      },
      {
        label: "RAM %",
        data: [],
        borderColor: "#a78bfa",
        backgroundColor: "rgba(167,139,250,0.06)",
        borderWidth: 1.5,
        tension: 0.3,
        fill: true,
        pointRadius: 0,
      },
    ],
  },
  options: {
    responsive: true,
    maintainAspectRatio: false,
    animation: { duration: 400 },
    scales: {
      y: {
        beginAtZero: true,
        max: 100,
        grid: { color: "rgba(255,255,255,0.04)" },
        ticks: { color: "#4a4f62", font: { family: "JetBrains Mono", size: 10 }, stepSize: 25 },
        border: { display: false },
      },
      x: {
        grid: { display: false },
        ticks: { color: "#4a4f62", font: { family: "JetBrains Mono", size: 10 }, maxTicksLimit: 5 },
        border: { display: false },
      },
    },
    plugins: {
      legend: {
        labels: {
          color: "#7c8296",
          font: { family: "Inter", size: 11 },
          boxWidth: 10,
          usePointStyle: true,
          pointStyle: "circle",
        },
      },
      tooltip: {
        backgroundColor: "#16191f",
        borderColor: "rgba(255,255,255,0.08)",
        borderWidth: 1,
        titleColor: "#7c8296",
        bodyFont: { family: "JetBrains Mono" },
        padding: 10,
      },
    },
  },
});

socket.on("history", (data) => {
  const labels = data.map((row) =>
    new Date(row.timestamp).toLocaleTimeString("es-AR", { hour12: false }),
  );
  const cpuData = data.map((row) => row.cpu);
  const ramData = data.map((row) => row.ram);
  historyChart.data.labels = labels;
  historyChart.data.datasets[0].data = cpuData;
  historyChart.data.datasets[1].data = ramData;
  historyChart.update();
});

/* ── Utilidades ──────────────────────────────── */
function formatSpeed(bytesPerSec) {
  if (!bytesPerSec || bytesPerSec === 0) return "0 B/s";
  const k = 1024;
  const sizes = ["B/s", "KB/s", "MB/s", "GB/s"];
  const i = Math.floor(Math.log(bytesPerSec) / Math.log(k));
  return parseFloat((bytesPerSec / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

/* ── Procesos ────────────────────────────────── */
let currentView = "cpu";
let lastProcData = { topCpu: [], topMem: [] };

document.getElementById("btn-cpu").onclick = () => switchView("cpu");
document.getElementById("btn-mem").onclick = () => switchView("mem");

function switchView(view) {
  currentView = view;
  document.getElementById("btn-cpu").classList.toggle("active", view === "cpu");
  document.getElementById("btn-mem").classList.toggle("active", view === "mem");
  document.getElementById("th-value").innerText = view === "cpu" ? "CPU %" : "RAM %";
  updateTable();
}

function updateTable() {
  const tableBody = document.getElementById("proc-body");
  const data = currentView === "cpu" ? lastProcData.topCpu : lastProcData.topMem;
  tableBody.innerHTML = data
    .map(
      (p) => `
        <tr>
          <td>${p.pid}</td>
          <td>${p.name}</td>
          <td>${(currentView === "cpu" ? p.cpu : p.mem).toFixed(1)}%</td>
        </tr>
      `,
    )
    .join("");
}

/* ── Socket stats ────────────────────────────── */
socket.on("stats", (data) => {
  const now = new Date().toLocaleTimeString("es-AR", { hour12: false });

  /* CPU chart */
  cpuChart.data.labels.push(now);
  cpuChart.data.datasets[0].data.push(data.cpu);
  if (cpuChart.data.labels.length > 20) {
    cpuChart.data.labels.shift();
    cpuChart.data.datasets[0].data.shift();
  }
  cpuChart.update();

  /* CPU stat card */
  const cpuVal = document.getElementById("stat-cpu-val");
  const cpuBar = document.getElementById("stat-cpu-bar");
  if (cpuVal) cpuVal.textContent = data.cpu.toFixed(1) + "%";
  if (cpuBar) cpuBar.style.width = data.cpu + "%";

  /* RAM */
  const ramPercent = (data.ramUsed / data.ramTotal) * 100;
  const ramBar  = document.getElementById("ram-bar");
  const ramText = document.getElementById("ram-text");
  const ramVal  = document.getElementById("stat-ram-val");

  if (ramVal)  ramVal.textContent  = data.ramUsed.toFixed(1) + " GB";
  if (ramText) ramText.textContent = `${data.ramUsed.toFixed(1)} / ${data.ramTotal.toFixed(1)} GB · ${ramPercent.toFixed(1)}%`;
  if (ramBar) {
    ramBar.style.width = ramPercent + "%";
    ramBar.className = "stat-bar-fill " + (ramPercent > 90 ? "stat-bar-fill--danger" : "stat-bar-fill--blue");
  }

  /* Red */
  document.getElementById("net-down").textContent = formatSpeed(data.netDownload);
  document.getElementById("net-up").textContent   = formatSpeed(data.netUpload);

  /* Batería */
  const battLevel  = document.getElementById("batt-level");
  const battStatus = document.getElementById("batt-status");
  const battBar    = document.getElementById("batt-bar");

  if (data.batteryPercent !== -1) {
    battLevel.textContent  = data.batteryPercent;
    battStatus.textContent = data.isCharging ? "⚡" : "🔋";
    if (battBar) battBar.style.width = data.batteryPercent + "%";
  } else {
    battLevel.textContent  = "N/A";
    battStatus.textContent = "";
    if (battBar) battBar.style.width = "0%";
  }

  /* Procesos */
  lastProcData.topCpu = data.topCpu;
  lastProcData.topMem = data.topMem;
  updateTable();
});
