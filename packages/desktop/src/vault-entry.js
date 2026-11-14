/* This file runs only in the dedicated sandboxed native Logins window. */
const field = (id) => document.getElementById(id);
let kind = "";
window.privateLogin.init((input) => {
  kind = input.kind;
  const entry = input.entry;
  const text = kind === "entry" ? ["Save login", "Enter this website login on the host. Secrets never enter the ordinary app view.", "Save login"]
    : kind === "fill" ? ["Fill login", `Fill ${entry.name} in this agent's browser? Verify the exact destination below.`, "Fill login"]
    : kind === "delete" ? ["Delete login", `Delete ${entry.name} from this host's protected store? Browser sign-ins and profiles stay in place.`, "Delete login"]
    : kind === "finish" ? ["Finish private interaction", input.mode === "close" ? "Close this agent's browser to remove the private page from memory. Its saved browser profile remains." : "Clear the login fields before leaving private mode. Browser automation remains paused until you explicitly return control.", input.mode === "close" ? "Close private browser" : "Clear fields"]
    : ["Unlock Logins", "Allow user-confirmed login entry and fills on this host for this session. Restarting or locking the app locks Logins again.", "Unlock Logins"];
  field("title").textContent = text[0]; field("description").textContent = text[1]; field("confirm").textContent = text[2];
  field("destination").textContent = input.destination || entry?.origin || "";
  if (kind === "entry") {
    field("fields").hidden = false; field("name").value = input.name; field("origin").value = input.origin;
    for (const id of ["name", "origin", "username", "password"]) field(id).required = true;
    field("name").focus();
  } else field("confirm").focus();
});
field("cancel").addEventListener("click", () => window.privateLogin.submit(null));
field("form").addEventListener("submit", async (event) => {
  event.preventDefault();
  let value = true;
  if (kind === "entry") {
    value = { name: field("name").value, origin: field("origin").value, username: field("username").value, password: field("password").value };
    field("username").value = ""; field("password").value = "";
  }
  field("confirm").disabled = true;
  try { await window.privateLogin.submit(value); }
  catch { field("problem").textContent = "Check the login fields and enter the secret again."; field("confirm").disabled = false; }
});
