// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const authKey = "seismograph.monitor.token";
const hash = location.hash.slice(1);
export const token = /^[a-f0-9]{64}$/.test(hash) ? hash : sessionStorage.getItem(authKey);
if (hash && token === hash) {
  sessionStorage.setItem(authKey, token);
  history.replaceState(null, "", "/");
}

async function request(path, { method = "GET", data, signal } = {}) {
  const headers = { Authorization: `Bearer ${token}` };
  if (data !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(`/api/v1${path}`, {
    method, signal, headers, body: data === undefined ? undefined : JSON.stringify(data),
  });
  return (await checked(response)).json();
}

async function checked(response) {
  if (!response.ok) {
    const body = await response.text();
    let message = body;
    if (response.headers.get("content-type")?.includes("application/json")) {
      try {
        const status = JSON.parse(body);
        message = status.message ?? status.error?.message ?? body;
      } catch {
        message = `Request failed (${response.status}).`;
      }
    }
    throw Object.assign(new Error(message || `Request failed (${response.status}).`), { status: response.status });
  }
  return response;
}

const application = id => `/applications/${encodeURIComponent(id)}`;
const snapshot = id => `/snapshots/${encodeURIComponent(id)}`;
export const api = {
  discover: signal => request("/applications", { signal }),
  activity: (id, signal) => request(`${application(id)}/activity`, { signal }),
  record: (id, data) => request(`${application(id)}/recording`, { method: "PUT", data }),
  clear: id => request(`${application(id)}/clear`, { method: "POST" }),
  capture: (id, mode) => request(`${application(id)}/capture`, { method: "POST", data: { mode } }),
  inspect: (id, viewId, section, offset, parent, fullStack, sortBy, descending, signal) => {
    const query = new URLSearchParams({ viewId, offset, parent, fullStack, sortBy, descending });
    return request(`${snapshot(id)}/sections/${encodeURIComponent(section)}?${query}`, { signal });
  },
  filter: (id, data) => request(`${snapshot(id)}/filter`, { method: "PUT", data }),
};

export async function upload(file) {
  if (file.size > 64 * 1024 * 1024) throw new Error("Open a snapshot of at most 64 MiB.");
  const response = await fetch("/api/v1/snapshots/native", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/octet-stream",
      "X-Snapshot-Name": file.name.replace(/[^\x20-\x7e]/g, "_").slice(0, 255),
    },
    body: file,
  });
  return (await checked(response)).json();
}

export async function download(id, name) {
  const response = await fetch(`/api/v1${snapshot(id)}/native`, { headers: { Authorization: `Bearer ${token}` } });
  const url = URL.createObjectURL(await (await checked(response)).blob());
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
