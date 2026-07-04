# vesper

Vesper is a language server for [dusk](https://github.com/choice404/dusk), a small systems language that compiles to native code through textual LLVM IR. It links the dusk compiler front end directly, so the diagnostics, highlighting, and navigation you see in your editor come from the same lexer, parser, and checker that build your program.

Vesper speaks the Language Server Protocol over stdio, so it works in any editor that speaks LSP. Neovim and VS Code are the first targets.

---

## Status

Vesper is early, and it tracks dusk, which is pre 1.0 and still moving, so expect rough edges.

- [x] Diagnostics, live as you type for syntax and on save for names and types
- [x] Semantic token highlighting
- [x] Hover, go to definition, and find references
- [x] Document and workspace symbols
- [ ] A tree-sitter grammar for baseline coloring
- [ ] Prebuilt binaries, a VS Code extension, and a mason entry

---

## How It Works

Vesper holds each open file in memory and runs two passes.

- **As you type**, it lexes and parses the buffer and gates its paradigm. This needs no other file, so it is fast and runs on every edit.
- **On open and save**, it loads the file and everything it imports, then resolves names and checks types across the whole program. This reads from disk, so it runs when the buffer and the file on disk agree.

One module reaches into the dusk compiler. Everything else works in protocol terms, so when dusk changes, that module is the only thing vesper has to catch up on.

---

## Requirements

- A recent Rust toolchain to build from source.
- A dusk checkout, so vesper can find the standard library. Point `DUSK_HOME` at it, or pass `duskHome` in your editor's LSP settings.

Vesper never emits code, so it does not need clang or LLVM.

---

## Build

```sh
cargo build --release
```

The server binary lands at `target/release/vesper`.

---

## Neovim

Neovim 0.11 or later, since the setup uses the built in `vim.lsp.config` and `vim.lsp.enable`. Build vesper first (see Build), then use one of the setups below. Point `duskHome` at a dusk checkout so `@import std.*` resolves, unless the server can already find the standard library beside its binary.

### Plain config

```lua
vim.filetype.add({ extension = { dusk = "dusk" } })

vim.lsp.config("vesper", {
  cmd = { "/path/to/vesper" },
  filetypes = { "dusk" },
  root_markers = { "dusk.toml", ".git" },
  init_options = { duskHome = "/path/to/dusk" },
})
vim.lsp.enable("vesper")
```

Or add `editors/nvim` from this repo to your runtime path, which detects `.dusk` files and sets the comment string on its own, then call `require("vesper").setup({ cmd = { "/path/to/vesper" } })`.

### LazyVim

vesper is not in `nvim-lspconfig`, so register it yourself. Add a file under `~/.config/nvim/lua/plugins/`, for example `dusk.lua`:

```lua
return {
  {
    "neovim/nvim-lspconfig",
    opts = {
      servers = {
        vesper = {
          cmd = { "vesper" }, -- or the full path to the binary
          filetypes = { "dusk" },
          root_markers = { "dusk.toml", ".git" },
          init_options = { duskHome = vim.fn.expand("~/path/to/dusk") },
        },
      },
      setup = {
        vesper = function(_, opts)
          vim.filetype.add({ extension = { dusk = "dusk" } })
          vim.lsp.config("vesper", opts)
          vim.lsp.enable("vesper")
          return true
        end,
      },
    },
  },
}
```

The `setup` handler returns `true` so LazyVim does not also try to start vesper through `nvim-lspconfig`, which does not know it yet.

### Mason

vesper is not in the mason registry yet, so `:MasonInstall vesper` will not find it. Build it from source and point `cmd` at the binary, or put the binary on your `PATH` and leave `cmd = { "vesper" }`. Mason still manages your other tools as usual. A mason entry is on the roadmap, and once it lands you will install vesper like any other server.

Open a `.dusk` file and vesper attaches. Diagnostics show as you type and on save, and highlighting, hover, go to definition, find references, and document and workspace symbols all work.

VS Code support is coming.

---

## License

Dual licensed under MIT or Apache 2.0. Pick whichever one fits your use.
