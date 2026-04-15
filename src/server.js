const express = require("express");
const http = require("http");
const { Server } = require("socket.io");
const si = require("systeminformation");
const path = require("path");
const sqlite3 = require("sqlite3");
const { open } = require("sqlite");
let db;

const app = express();
const server = http.createServer(app);
const io = new Server(server);

(async () => {
  db = await open({
    filename: "./database.sqlite",
    driver: sqlite3.Database,
  });
  await db.exec(`
        CREATE TABLE IF NOT EXISTS stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cpu REAL,
            ram REAL,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
        )
    `);
  console.log("Base de datos lista");
})();

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

setInterval(async () => {
  if (db) {
    try {
      const cpu = await si.currentLoad();
      const ram = await si.mem();
      const ramPercent = (ram.active / ram.total) * 100;
      await db.run("INSERT INTO stats (cpu, ram) VALUES (?, ?)", [
        cpu.currentLoad.toFixed(2),
        ramPercent.toFixed(2),
      ]);
    } catch (e) {
      console.error("Error guardando en DB", e);
    }
  }
}, 60000);

io.on("connection", async (socket) => {
  console.log("Cliente conectado");
  if (db) {
    const history = await db.all(
      "SELECT * FROM stats ORDER BY timestamp DESC LIMIT 100",
    );
    socket.emit("history", history.reverse());
  }
});
