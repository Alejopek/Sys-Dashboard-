const socket = io();

socket.on("stats", (data) => {
  console.log("Datos recibidos del servidor:", data);
});

socket.on("connect", () => {
  console.log("Conectado al servidor de métricas");
});
