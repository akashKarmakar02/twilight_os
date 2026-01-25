function setText(id, text) {
  const el = document.getElementById(id);
  if (!el) return;
  el.textContent = text;
}

// Basic "status" card that doesn't depend on special endpoints.
setText("status-time", new Date().toLocaleString());

// If the server is up, fetching "/" should succeed.
fetch("/", { cache: "no-store" })
  .then((r) => {
    if (!r.ok) throw new Error(String(r.status));
    setText("status-http", "online");
  })
  .catch(() => setText("status-http", "offline"));
