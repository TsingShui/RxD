(function () {
  "use strict";

  var doc = document;

  // Hairline appears only once the page has moved.
  var nav = doc.querySelector("[data-nav]");
  if (nav) {
    var onScroll = function () {
      nav.classList.toggle("is-scrolled", window.scrollY > 12);
    };
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
  }

  // Reveal-on-scroll; content stays visible without IntersectionObserver.
  var targets = Array.prototype.slice.call(doc.querySelectorAll("[data-reveal]"));
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!("IntersectionObserver" in window) || reduce) {
    targets.forEach(function (el) { el.classList.add("is-in"); });
  } else {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-in");
          io.unobserve(entry.target);
        }
      });
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.08 });
    targets.forEach(function (el) { io.observe(el); });
  }

  // Copy buttons for install snippets.
  function legacyCopy(text) {
    var area = doc.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    doc.body.appendChild(area);
    area.select();
    var ok = false;
    try { ok = doc.execCommand("copy"); } catch (err) { ok = false; }
    doc.body.removeChild(area);
    return ok;
  }

  Array.prototype.forEach.call(doc.querySelectorAll("[data-copy]"), function (button) {
    var snippet = button.parentElement;
    var code = snippet && snippet.querySelector("pre code");
    if (!code) return;
    var label = button.textContent;
    var timer = 0;

    button.addEventListener("click", function () {
      var text = code.innerText.replace(/\s+$/, "");
      var done = function (ok) {
        button.textContent = ok ? "Copied" : "Copy failed";
        button.classList.toggle("is-copied", ok);
        window.clearTimeout(timer);
        timer = window.setTimeout(function () {
          button.textContent = label;
          button.classList.remove("is-copied");
        }, 1800);
      };

      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(function () { done(true); }, function () {
          done(legacyCopy(text));
        });
      } else {
        done(legacyCopy(text));
      }
    });
  });
})();
