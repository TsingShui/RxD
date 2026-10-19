// Assumes ordinary page objects and unmodified page intrinsics; Proxy traps may execute page code.
// Own data properties only at the replacement site; accessors are never probed.
// Saved references bypass this wrapper. Function identity changes; installation is not idempotent.
// Construction is forwarded unchanged, not observed. Late polling cannot guarantee the first call.
// Logs narrow setup stages, not crash causes or synchronous delivery before a process crash.
const __rxdApply = Reflect.apply;
const objectLike = value => value !== null && (typeof value === "object" || typeof value === "function");
function descriptor(object, key) {
  let owner = object;
  for (let depth = 0; owner !== null; depth++) {
    if (depth >= 256) throw new Error("unsupported prototype traversal depth");
    const property = Object.getOwnPropertyDescriptor(owner, key);
    if (property !== undefined) {
      if (!("value" in property)) throw new Error("unsupported accessor property");
      return { owner, property };
    }
    owner = Object.getPrototypeOf(owner);
  }
  return null;
}
function install() {
  __rxdStage = "web.target";
  __rxdLog("begin");
  let receiver = globalThis;
  for (let index = 0; index < keys.length - 1; index++) {
    const found = descriptor(receiver, keys[index]);
    if (found === null) return false;
    receiver = found.property.value;
    if (!objectLike(receiver)) throw new Error("path intermediate is not an object");
  }
  const key = keys[keys.length - 1];
  const found = descriptor(receiver, key);
  if (found === null) return false;
  const property = found.property;
  const original = property.value;
  if (typeof original !== "function") throw new Error("target is not callable");
  __rxdLog("success");
  __rxdStage = "web.replaceability";
  __rxdLog("begin");
  if (found.owner !== receiver) throw new Error("unsupported inherited final property; select its explicit owner");
  if (!property.configurable && !property.writable) throw new Error("target data property is not replaceable");
  __rxdLog("success");
  __rxdStage = "web.install";
  __rxdLog("begin");
  const wrapped = new Proxy(original, {
    apply(original, receiver, args) {
      __rxdLog("call", receiver, args);
      try {
        const result = __rxdApply(original, receiver, args);
        __rxdLog("return", result);
        return result;
      } catch (thrown) {
        __rxdLog("throw", thrown);
        throw thrown;
      }
    }
  });
  Object.defineProperty(receiver, key, { ...property, value: wrapped });
  const installed = Object.getOwnPropertyDescriptor(receiver, key);
  if (installed === undefined || installed.value !== wrapped ||
      installed.configurable !== property.configurable || installed.writable !== property.writable ||
      installed.enumerable !== property.enumerable) throw new Error("replacement verification failed");
  __rxdLog("success");
  return true;
}
