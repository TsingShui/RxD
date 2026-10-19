const api = "/api/login";
function request(user) {
  const send = async () => fetch("/api/login", { method: "POST", body: user });
  return send();
}
const handlers = {
  login() { return request("guest"); },
  get status() { return "ready"; },
  set status(value) { this.value = value; },
};
class Client {
  constructor(user) { this.user = user; }
  static async create() { return new Client("guest"); }
  *events() { yield "ready"; }
  run = () => request(this.user);
}
