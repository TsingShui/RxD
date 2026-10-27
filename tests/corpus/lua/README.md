# Lua corpus

`sample-5.4.luac` is the plain PUC Lua 5.4 bytecode for the script below,
compiled by the reference compiler with default options (debug info kept):

```lua
local secret = "authorization-token"
local function greet(name)
  return secret .. ":" .. name
end
return greet
```

Regenerate with the pinned upstream sources:

```sh
curl -sSL https://www.lua.org/ftp/lua-5.4.7.tar.gz | tar xz
make -C lua-5.4.7 -j macosx
lua-5.4.7/src/luac -o sample-5.4.luac sample.lua
```

Lua is MIT-licensed; the bytecode here is produced from the script in this file,
not copied from another project.
