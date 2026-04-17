const socket = io();

const ctx = document.getElementById("cpuChart").getContext("2d");
const cpuChart = new Chart(ctx, {
  type: "line",
  data: {
    labels: [],
    datasets: [
      {
        label: "Uso de CPU (%)",
        data: [],
        borderColor: "#00ff88",
        backgroundColor: "rgba(0, 255, 136, 0.1)",
        borderWidth: 2,
        tension: 0.4,
        fill: true,
      },
    ],
  },
  options: {
    responsive: true,
    scales: {
      y: {
        beginAtZero: true,
        max: 100,
        grid: { color: "#333" },
      },
      x: {
        grid: { display: false },
      },
    },
    plugins: {
      legend: { labels: { color: "#fff" } },
    },
  },
});

const ctxHistory = document.getElementById("historyChart").getContext("2d");
const historyChart = new Chart(ctxHistory, {
  type: "line",
  data: {
    labels: [],
    datasets: [
      {
        label: "Promedio CPU (%)",
        data: [],
        borderColor: "#00bdff",
        tension: 0.1,
        fill: false,
      },
      {
        label: "Promedio RAM (%)",
        data: [],
        borderColor: "#ff0088",
        tension: 0.1,
        fill: false,
      },
    ],
  },
  options: {
    responsive: true,
    plugins: { legend: { labels: { color: "#fff" } } },
  },
});

socket.on("history", (data) => {
  const labels = data.map((row) =>
    new Date(row.timestamp).toLocaleTimeString(),
  );
  const cpuData = data.map((row) => row.cpu);
  const ramData = data.map((row) => row.ram);
  historyChart.data.labels = labels;
  historyChart.data.datasets[0].data = cpuData;
  historyChart.data.datasets[1].data = ramData;
  historyChart.update();
});

function formatSpeed(bytesPerSec) {
  if (bytesPerSec === 0) return "0 B/s";
  const k = 1024;
  const sizes = ["B/s", "KB/s", "MB/s", "GB/s"];
  const i = Math.floor(Math.log(bytesPerSec) / Math.log(k));
  return parseFloat((bytesPerSec / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
}

let currentView = "cpu";
let lastProcData = { topCpu: [], topMem: [] };

document.getElementById("btn-cpu").onclick = () => switchView("cpu");
document.getElementById("btn-mem").onclick = () => switchView("mem");
function switchView(view) {
  currentView = view;
  document.getElementById("btn-cpu").classList.toggle("active", view === "cpu");
  document.getElementById("btn-mem").classList.toggle("active", view === "mem");
  document.getElementById("th-value").innerText =
    view === "cpu" ? "CPU %" : "RAM %";
  updateTable();
}
function updateTable() {
  const tableBody = document.getElementById("proc-body");
  const data =
    currentView === "cpu" ? lastProcData.topCpu : lastProcData.topMem;

  tableBody.innerHTML = data
    .map(
      (p) => `
        <tr>
            <td>${p.pid}</td>
            <td>${p.name}</td>
            <td>${currentView === "cpu" ? p.cpu.toFixed(1) : p.mem.toFixed(1)}%</td>
        </tr>
    `,
    )
    .join("");
}

socket.on("stats", (data) => {
  const now = new Date().toLocaleTimeString();

  cpuChart.data.labels.push(now);
  cpuChart.data.datasets[0].data.push(data.cpu);

  if (cpuChart.data.labels.length > 20) {
    cpuChart.data.labels.shift();
    cpuChart.data.datasets[0].data.shift();
  }

  cpuChart.update();

  const ramPercent = (data.ramUsed / data.ramTotal) * 100;
  document.getElementById("ram-bar").style.width = `${ramPercent}%`;
  document.getElementById("ram-text").innerText =
    `${data.ramUsed}GB / ${data.ramTotal}GB (${ramPercent.toFixed(1)}%)`;

  if (ramPercent > 90) {
    document.getElementById("ram-bar").style.background = "#ff4a4a";
  } else {
    document.getElementById("ram-bar").style.background =
      "linear-gradient(90deg, #00ff88, #00bdff)";
  }

  document.getElementById("net-down").innerText = formatSpeed(data.netDownload);
  document.getElementById("net-up").innerText = formatSpeed(data.netUpload);
  if (data.batteryPercent !== -1) {
    document.getElementById("batt-level").innerText = data.batteryPercent;
    document.getElementById("batt-status").innerText = data.isCharging
      ? "⚡"
      : "🔋";
  } else {
    document.getElementById("batt-level").innerText = "N/A";
  }
  lastProcData.topCpu = data.topCpu;
  lastProcData.topMem = data.topMem;
  updateTable();
});
