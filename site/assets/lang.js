(function () {
  "use strict";

  var path = location.pathname;
  var isZh = /(^|\/)zh(\/|$)/.test(path);
  var file = path.charAt(path.length - 1) === "/" ? "index.html" : path.split("/").pop();
  if (!/\.html?$/.test(file)) file = "index.html";

  var forced = new URLSearchParams(location.search).get("lang");
  var stored = null;
  try { stored = localStorage.getItem("rxd-lang"); } catch (error) {}

  var language = (navigator.languages && navigator.languages[0]) || navigator.language || "en";
  var want;
  if (forced === "zh" || forced === "en") want = forced;
  else if (stored === "zh" || stored === "en") want = stored;
  else want = String(language).toLowerCase().indexOf("zh") === 0 ? "zh" : "en";

  if (forced === "zh" || forced === "en") {
    try { localStorage.setItem("rxd-lang", forced); } catch (error) {}
  }

  var hash = location.hash || "";
  if (want === "zh" && !isZh) {
    location.replace("zh/" + file + (forced ? "?lang=zh" : "") + hash);
  } else if (want === "en" && isZh) {
    location.replace("../" + file + (forced ? "?lang=en" : "") + hash);
  }
})();
