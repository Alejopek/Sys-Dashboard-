const express = require("express");
const http = require("http");
const { Server } = require("socket.io");
const si = require("systeminformation");
const path = require("path");

const app = express();
const server = http.createServer(app);
const io = new Server(server);

app.use(express.static(path.join(__dirname, "../public")));

setInterval(async () => {
  try {
    const cpu = await si.currentLoad();
    const ram = await si.mem();

    io.emit("stats", {
      cpu: cpu.currentLoad.toFixed(2),
      ramUsed: (ram.active / 1024 / 1024 / 1024).toFixed(2),
      ramTotal: (ram.total / 1024 / 1024 / 1024).toFixed(2),
    });
  } catch (error) {
    console.error("Error obteniendo datos:", error);
  }
}, 1000);

const PORT = 3000;
server.listen(PORT, () => {
  console.log(`🚀 Servidor corriendo en http://localhost:${PORT}`);
});
