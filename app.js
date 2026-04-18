const { app, BrowserWindow } = require("electron");
const path = require("path");
const server = require("./src/server.js");

const userDataPath = app.getPath("userData");

process.env.DATABASE_PATH = path.join(userDataPath, "database.sqlite");
require("./src/server.js");

function createWindow() {
  const win = new BrowserWindow({
    width: 1000,
    height: 800,
    webPreferences: {
      nodeIntegration: false,
    },
    autoHideMenuBar: true,
    icon: path.join(__dirname, "public/icon.png"),
  });

  win.loadURL("http://localhost:3000");
}

app.whenReady().then(createWindow);

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
