-- veil-host configuration file
-- Place at ./config.lua or ~/.config/veil/config.lua

quality = "auto"         -- auto | kitty | pixel | ascii | ascii_edge
fps = 60                 -- compositor frame rate cap
cage_timeout_secs = 8    -- timeout for caged output
input = true             -- enable input forwarding
gpu_render = true        -- use GPU for frame encoding

-- Layout keybinds: (Super key is default mod key)
keybinds = {
  mod_key = "super",     -- shift | ctrl | alt | super
  focus_left = "h",      -- Super+H
  focus_right = "l",     -- Super+L
  focus_up = "k",        -- Super+K
  focus_down = "j",      -- Super+J
  swap = "s",            -- Super+S (swap with neighbor)
  rotate = "r",          -- Super+R (rotate split axis)
  close = "q",           -- Super+Q (close window)
}
