// Inkvec Studio Lite's loading screen, before the app's bundle arrives. Inlined into the
// head of index.html (and allowed by its own hash in the page's content policy), so it runs
// before the first paint:
//
// - `framed` when the page is inside another page's frame (Hugging Face shows the Space in
//   one), so the loading screen sits below the same gap and hairline the app will;
// - `boot-go` once the two faces have loaded (or 700 ms have passed), which starts the
//   entrance, so it never plays on a fallback font;
// - a failsafe: if the app's own script has not taken the screen over in 30 s (blocked, or
//   a network that dropped it), say so and offer a reload instead of a bar that never moves.
//
// Everything after that is lib/web/chrome.ts: real progress, and the hand-over.
(function () {
  var root = document.documentElement;
  var framed = false;
  try {
    framed = window.self !== window.top;
  } catch (e) {
    framed = true;
  }
  if (framed) root.classList.add("framed");

  var go = function () {
    root.classList.add("boot-go");
  };
  try {
    Promise.race([
      Promise.all([document.fonts.load('500 60px "Playfair Studio"'), document.fonts.load('400 13px "Inter Studio"')]),
      new Promise(function (resolve) {
        setTimeout(resolve, 700);
      }),
    ]).then(go, go);
  } catch (e) {
    go();
  }

  setTimeout(function () {
    var boot = document.getElementById("boot");
    if (!boot || window.__inkvecBoot) return;
    boot.classList.add("boot-failed");
    var status = document.getElementById("boot-status");
    var detail = document.getElementById("boot-detail");
    if (status) status.textContent = "The Studio did not load";
    if (detail) {
      detail.textContent = "Its script never arrived. Check the connection, then reload.";
      var b = document.createElement("button");
      b.className = "boot-reload";
      b.textContent = "Reload";
      b.onclick = function () {
        location.reload();
      };
      detail.parentNode.appendChild(b);
    }
  }, 30000);
})();
