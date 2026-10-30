// sheprd-opencode: tells sheprd which model an OpenCode agent uses (SHE-100011).
// installed by `sheprd setup`; reinstalling overwrites this file.
//
// An OpenCode TUI plugin. While OpenCode runs in a herdr pane it reports the model its prompt is
// set to (what the footer shows, e.g. "claude-agent/opus") as the pane metadata token
// `sheprd_model`, which `sheprd msg list`, the work panel and the shared view show. Only when it
// changes; a failed report is retried on the next change check. Outside herdr it does nothing.
// OpenCode 2 (`setup`) only: OpenCode 1's TUI API does not expose the selected model.

import net from "node:net";

const CHECK_EVERY_MS = 2_000;
const MAX_VALUE = 80;

function report(model) {
  const socketPath = process.env.HERDR_SOCKET_PATH;
  const request = {
    id: `sheprd:opencode:${Date.now()}`,
    method: "pane.report_metadata",
    params: {
      pane_id: process.env.HERDR_PANE_ID,
      source: "sheprd",
      tokens: { sheprd_model: model },
    },
  };
  return new Promise((resolve) => {
    let settled = false;
    const settle = (ok) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.destroy();
      resolve(ok);
    };
    const endpoint = process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
    const client = net.createConnection(endpoint, () => client.write(`${JSON.stringify(request)}\n`));
    const timer = setTimeout(() => settle(false), 500);
    timer.unref?.();
    client.on("data", (data) => settle(!String(data).includes('"error"')));
    client.on("error", () => settle(false));
    client.on("close", () => settle(false));
  });
}

// "provider/model", as OpenCode's own config names it (`model = "claude-agent/opus"`).
export function modelName(current) {
  if (!current?.providerID || !current?.modelID) return undefined;
  return `${current.providerID}/${current.modelID}`.slice(0, MAX_VALUE);
}

function setup(api) {
  if (process.env.HERDR_ENV !== "1" || !process.env.HERDR_SOCKET_PATH || !process.env.HERDR_PANE_ID) return;
  let reported;
  let busy = false;
  const check = async () => {
    if (busy) return;
    let model;
    try {
      model = modelName(api.ui.model.current());
    } catch {
      return;
    }
    if (!model || model === reported) return;
    busy = true;
    try {
      if (await report(model)) reported = model;
    } finally {
      busy = false;
    }
  };
  const timer = setInterval(check, CHECK_EVERY_MS);
  timer.unref?.();
  void check();
  return () => clearInterval(timer);
}

export default { id: "sheprd.opencode", setup };
