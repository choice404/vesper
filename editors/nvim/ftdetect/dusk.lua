-- Map the .dusk extension to the dusk filetype, so buffers are detected as soon
-- as this directory is on the runtime path.
vim.filetype.add({ extension = { dusk = "dusk" } })
