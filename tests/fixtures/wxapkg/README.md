# wxapkg fixtures

`sample.wxapkg` is a handcrafted normal package with `app-config.json`,
`app-service.js` and `pages/index/index.wxml`.

`sample.enc.wxapkg` is the same package encrypted with the WeChat PC `V1MMWX`
layout for appid `wx1234567890abcdef` (AES-256-CBC plus XOR tail).

Both are small parser fixtures, not production packages. Their expected payloads
are asserted by `tests/unpack/mod.rs`.
