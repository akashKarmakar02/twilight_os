const out = document.getElementById("out");
document.getElementById("btn").addEventListener("click", () => {
  out.textContent = `clicked at ${Date.now()}`;
});

