// Onboarding window. Dismisses itself when the user either clicks the on-screen
// Insert key OR presses Insert on their keyboard. By this point aegis is
// already running in the background (spawned when the cursor was clicked on
// welcome), so closing the window leaves them with just the agent.

function dismiss() {
  window.__TAURI__.window.getCurrentWindow().close();
}

// Click the visual key.
document.getElementById("insert-key").addEventListener("click", dismiss);

// Or press the physical Insert key while this window has focus.
window.addEventListener("keydown", (e) => {
  if (e.key === "Insert") dismiss();
});
