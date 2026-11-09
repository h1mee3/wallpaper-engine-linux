use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;


pub struct NoctaliaIntegration;

impl NoctaliaIntegration {
    /// Return the target directory for the Noctalia wengine plugin
    pub fn plugin_dir() -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        // Official source cache directory which Noctalia actively scans
        let target = home.join(".local/state/noctalia/plugins/sources/official/repo/wengine");
        Some(target)
    }

    /// Install or update the Noctalia plugin files
    pub fn install() -> Result<PathBuf> {
        let dir = Self::plugin_dir().context("Could not determine Noctalia plugin directory")?;
        fs::create_dir_all(&dir)?;

        // 1. plugin.toml
        let plugin_toml = r#"id = "noctalia/wengine"
name = "Wallpaper Engine"
version = "1.0.0"
plugin_api = 30
author = "wengine"
license = "MIT"
dependencies = []
tags = ["wallpaper", "wallpaper-engine", "video"]
icon = "image"
description = "Wallpaper Engine Linux integration for Hyprland & Noctalia"

[[service]]
id = "service"
entry = "service.luau"

[[widget]]
id = "wengine"
entry = "widget.luau"

[[panel]]
id = "picker"
entry = "panel.luau"
width = 920
height = 740
placement = "attached"
position = "auto"
"#;
        fs::write(dir.join("plugin.toml"), plugin_toml)?;

        // 2. widget.luau
        let widget_luau = r#"--!nonstrict
-- Wallpaper Engine bar widget for Noctalia
local function render()
  barWidget.setGlyph("image")
  barWidget.setTooltip("Wallpaper Engine")
end

function update()
  render()
end

function onClick()
  noctalia.togglePanel("noctalia/wengine:picker")
end

function onRightClick()
  -- Toggle pause via CLI
  noctalia.runAsync({ "wengine", "toggle" }, function() end)
end

render()
"#;
        fs::write(dir.join("widget.luau"), widget_luau)?;

        // 3. service.luau
        let service_luau = r#"--!nonstrict
-- Wallpaper Engine background service for Noctalia
noctalia.setUpdateInterval(2000)

local function syncStatus()
  noctalia.runAsync({ "wengine", "status", "--json" }, function(res)
    if res.exitCode == 0 and res.stdout ~= "" then
      local data = noctalia.json.decode(res.stdout)
      if data then
        noctalia.state.wengine_status = data
      end
    end
  end)
end

function update()
  syncStatus()
end

syncStatus()
"#;
        fs::write(dir.join("service.luau"), service_luau)?;

        // 4. panel.luau
        let panel_luau = r#"--!nonstrict
-- Wallpaper Engine Picker Panel for Noctalia
local PANEL_WIDTH = 900
local COLUMNS = 4
local COLUMN_GAP = 12
local TILE_RADIUS = 10

local wallpapers = {}
local searchQuery = ""
local selectedConnector = "DP-1"
local activeFilter = "all" -- all, video, scene

local function fetchWallpapers()
  noctalia.runAsync({ "wengine", "list", "--json" }, function(res)
    if res.exitCode == 0 and res.stdout ~= "" then
      local data = noctalia.json.decode(res.stdout)
      if data then
        wallpapers = data
        panel.requestRender()
      end
    end
  end)
end

local function applyWallpaper(id)
  local target = selectedConnector == "all" and "*" or selectedConnector
  noctalia.runAsync({ "wengine", "apply", id, "--monitor", target }, function(res)
    fetchWallpapers()
  end)
end

local function filteredWallpapers()
  local list = {}
  local query = searchQuery:lower()
  for _, wp in ipairs(wallpapers) do
    local matchFilter = true
    if activeFilter ~= "all" then
      if activeFilter == "video" and wp.wallpaper_type ~= "video" then
        matchFilter = false
      elseif activeFilter == "scene" and wp.wallpaper_type ~= "scene" then
        matchFilter = false
      end
    end

    if matchFilter then
      if query == "" or wp.title:lower():find(query, 1, true) then
        table.insert(list, wp)
      end
    end
  end
  return list
end

local function wallpaperTile(wp)
  local frameWidth = math.floor((PANEL_WIDTH - (COLUMNS + 1) * COLUMN_GAP) / COLUMNS)
  local frameHeight = math.floor(frameWidth * 9 / 16)

  local preview
  if wp.preview_path and wp.preview_path ~= "" then
    preview = ui.image({
      width = frameWidth,
      height = frameHeight,
      path = wp.preview_path,
      fit = "cover",
      radius = TILE_RADIUS,
      onClick = function() applyWallpaper(wp.id) end,
    })
  else
    preview = ui.box({
      width = frameWidth,
      height = frameHeight,
      fill = "surface_container",
      radius = TILE_RADIUS,
      onClick = function() applyWallpaper(wp.id) end,
    })
  end

  local typeBadge = ui.label({
    text = "[" .. string.upper(wp.wallpaper_type or "scene") .. "]",
    fontSize = 10,
    color = wp.wallpaper_type == "video" and "primary" or "secondary",
  })

  return ui.column({ gap = 4, align = "center" }, {
    preview,
    ui.row({ gap = 6, align = "center" }, {
      typeBadge,
      ui.label({
        text = wp.title,
        fontSize = 11,
        maxWidth = frameWidth - 50,
        maxLines = 1,
      }),
    }),
  })
end

function render()
  local items = filteredWallpapers()
  local rows = {}
  local currentRow = {}

  for i, wp in ipairs(items) do
    table.insert(currentRow, wallpaperTile(wp))
    if #currentRow == COLUMNS then
      table.insert(rows, ui.row({ gap = COLUMN_GAP, justify = "center" }, currentRow))
      currentRow = {}
    end
  end
  if #currentRow > 0 then
    table.insert(rows, ui.row({ gap = COLUMN_GAP, justify = "flex-start" }, currentRow))
  end

  local header = ui.row({ gap = 12, align = "center", justify = "space-between" }, {
    ui.label({ text = "Wallpaper Engine Library (" .. #items .. ")", fontSize = 16, bold = true }),
    ui.row({ gap = 8 }, {
      ui.button({
        text = "Toggle Pause",
        onClick = function()
          noctalia.runAsync({ "wengine", "toggle" }, function() end)
        end,
      }),
      ui.button({
        text = "Random",
        onClick = function()
          noctalia.runAsync({ "wengine", "random" }, function() fetchWallpapers() end)
        end,
      }),
    }),
  })

  local controls = ui.row({ gap = 12, align = "center" }, {
    ui.button({
      text = "All (" .. #wallpapers .. ")",
      variant = activeFilter == "all" and "filled" or "outlined",
      onClick = function() activeFilter = "all"; panel.requestRender() end,
    }),
    ui.button({
      text = "Videos",
      variant = activeFilter == "video" and "filled" or "outlined",
      onClick = function() activeFilter = "video"; panel.requestRender() end,
    }),
    ui.button({
      text = "Scenes",
      variant = activeFilter == "scene" and "filled" or "outlined",
      onClick = function() activeFilter = "scene"; panel.requestRender() end,
    }),
  })

  return ui.column({ gap = 16, padding = 20 }, {
    header,
    controls,
    ui.column({ gap = 12 }, rows),
  })
end

fetchWallpapers()
"#;
        fs::write(dir.join("panel.luau"), panel_luau)?;

        info!("Installed Noctalia plugin to {:?}", dir);

        // Try enabling the plugin in Noctalia
        let _ = Command::new("noctalia")
            .args(["msg", "plugins", "enable", "noctalia/wengine"])
            .output();

        Ok(dir)
    }
}
