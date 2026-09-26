vim.opt.termguicolors = true
vim.opt.number = true
vim.opt.cursorline = true
vim.opt.signcolumn = "yes"
vim.opt.laststatus = 3
vim.opt.showmode = false
vim.opt.fillchars = { eob = " " }
vim.opt.shortmess:append("I")
vim.cmd.colorscheme("default")
-- Let the terminal's translucent background through.
for _, group in ipairs({ "Normal", "NormalNC", "SignColumn", "LineNr", "EndOfBuffer" }) do
  vim.api.nvim_set_hl(0, group, { bg = "NONE", fg = group == "LineNr" and "#585b70" or nil })
end
vim.api.nvim_set_hl(0, "CursorLine", { bg = "#2a2b3c" })
vim.api.nvim_set_hl(0, "CursorLineNr", { fg = "#cba6f7", bold = true })
vim.api.nvim_set_hl(0, "StatusLine", { bg = "#313244", fg = "#cdd6f4" })
vim.opt.statusline = " %#CursorLineNr# NORMAL %* %f %= rust  %l:%c "
