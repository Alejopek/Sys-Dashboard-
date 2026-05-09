const { app, BrowserWindow } = require("electron");
const path = require("path");
const { startServer } = require("./src/server.js");

const userDataPath = app.getPath("userData");
process.env.DATABASE_PATH = path.join(userDataPath, "database.sqlite");

async function createWindow() {
  try {
    const port = await startServer();

    const win = new BrowserWindow({
      width: 1000,
      height: 800,
      webPreferences: {
        nodeIntegration: false,
        contextIsolation: true,
      },
      autoHideMenuBar: true,
      icon: path.join(__dirname, "public/icon.png"),
    });

    win.loadURL(`http://localhost:${port}`);
  } catch (err) {
    console.error("Failed to start server:", err);
    app.quit();
  }
}

app.whenReady().then(createWindow);

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
