let timer = null;
let deadline;
function cleanup() {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
}
function attempt() {
  try {
    __rxdStage = "web.wait";
    const remaining = deadline - performance.now();
    if (remaining <= 0) {
      __rxdLog("timeout");
      throw new Error("target wait expired");
    }
    if (install()) {
      cleanup();
      return;
    }
    __rxdStage = "web.wait";
    timer = setTimeout(attempt, Math.min(25, remaining));
  } catch (error) {
    cleanup();
    __rxdLog("failure", error);
    throw error;
  }
}
try {
  __rxdStage = "web.wait";
  __rxdLog("waiting", waitMs);
  deadline = performance.now() + waitMs;
} catch (error) {
  __rxdLog("failure", error);
  throw error;
}
attempt();
