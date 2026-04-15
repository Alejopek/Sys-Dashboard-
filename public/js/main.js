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
});
