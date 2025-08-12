file-sharing-example:
	@$(MAKE) file-sharing-provider & $(shell sleep 1)
	@$(MAKE) file-sharing-getter

file-sharing-provider:
	@cargo run --bin file-sharing-example -- --listen-address /ip4/127.0.0.1/tcp/40837 --secret-key-seed 1 provide --path README.md --name README.md 2>/dev/null

file-sharing-getter:
	@cargo -q run --bin file-sharing-example -- --peer /ip4/127.0.0.1/tcp/40837/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X get --name README.md 2>/dev/null
