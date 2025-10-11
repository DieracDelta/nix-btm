return {
	["rust-analyzer"] = {
		cargo = {
			target = "x86_64-unknown-linux-musl",
			buildScripts = { enable = true },
		},
		check = {
			command = "clippy",
			extraArgs = { "--target", "x86_64-unknown-linux-musl" },
		},
		procMacro = {
			enable = true,
		},
	},
}
