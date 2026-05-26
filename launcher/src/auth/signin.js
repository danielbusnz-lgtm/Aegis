// Auto-fit the Tauri window to the sign-in card. Reads the card's rendered
// box once Tailwind has applied its classes, then resizes the OS window to
// match plus a small margin. Means we never have to hardcode pixel numbers
// in tauri.conf.json: tweak the card and the window follows.
const MARGIN = 16;

async function fitWindowToCard() {
  const card = document.getElementById("signin-card");
  if (!card) return;
  const rect = card.getBoundingClientRect();
  const { getCurrentWindow, PhysicalSize } = window.__TAURI__.window;
  await getCurrentWindow().setSize(
    new PhysicalSize(
      Math.ceil(rect.width + MARGIN * 2),
      Math.ceil(rect.height + MARGIN * 2),
    ),
  );
}

// Tailwind via CDN applies styles after the script runs; wait one frame so
// the card has its final layout before we measure.
requestAnimationFrame(() => requestAnimationFrame(fitWindowToCard));

// Form handler. Real auth flows through the proxy backend once it exists.
document.getElementById("signin-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const email = document.getElementById("email").value;
  const password = document.getElementById("password").value;
  // TODO: post to /auth/signin on the proxy backend.
  console.log("[signin] would submit:", { email, passwordLen: password.length });
});
