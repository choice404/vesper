-- vesper: the dusk language server, for Neovim 0.11 and later.
--
-- With this directory on your runtime path, through a plugin manager or a plain
-- `:set runtimepath`, `.dusk` files are detected and the comment string is set.
-- Start the server with:
--
--   require("vesper").setup({
--     cmd = { "/path/to/vesper" },                    -- defaults to `vesper` on PATH
--     init_options = { duskHome = "/path/to/dusk" },  -- where the dusk `lib` lives
--   })
--
-- `duskHome` is only needed when the server cannot already find the standard
-- library. Point it at a dusk checkout so `@import std.*` resolves.

local M = {}

function M.setup(opts)
  opts = opts or {}
  -- Register the filetype here too, so a plain require without this directory on
  -- the runtime path still works.
  vim.filetype.add({ extension = { dusk = "dusk" } })

  vim.lsp.config("vesper", {
    cmd = opts.cmd or { "vesper" },
    filetypes = { "dusk" },
    root_markers = { "dusk.toml", ".git" },
    init_options = opts.init_options,
  })
  vim.lsp.enable("vesper")
end

return M
