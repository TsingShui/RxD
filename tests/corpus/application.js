// Handcrafted browser-client scenario. Transform and parse this file; never execute it.
function createClient(transport, clock) {
  const cache = new Map();
  class Client {
    constructor(origin) {
      this.origin = origin;
      this.requests = 0;
    }
    async login(user, password) {
      const body = JSON.stringify({ user, password });
      const send = async () => {
        const response = await transport(this.origin + "/api/login", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body,
        });
        if (!response.ok) throw new Error("Authentication failed");
        return response.json();
      };
      this.requests++;
      const result = await send();
      cache.set(user, { result, time: clock() });
      return result;
    }
    get status() { return `请求次数：${this.requests}`; }
    set timeout(value) { this.timeoutMs = value; }
    refresh = (user) => cache.get(user)?.result ?? null;
  }
  return new Client("https://example.test");
}
globalThis.clientFactory = createClient;
globalThis.handlers = {
  retry(client, user, password) {
    return client.login(user, password).catch(function rejected(error) {
      if (error.message.includes("Authentication failed")) return null;
      throw error;
    });
  },
};
