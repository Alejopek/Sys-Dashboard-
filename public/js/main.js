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

socket.on("stats", (data) => {
  const now = new Date().toLocaleTimeString();

  cpuChart.data.labels.push(now);
  cpuChart.data.datasets[0].data.push(data.cpu);

  if (cpuChart.data.labels.length > 20) {
    cpuChart.data.labels.shift();
    cpuChart.data.datasets[0].data.shift();
  }

  cpuChart.update();
});
