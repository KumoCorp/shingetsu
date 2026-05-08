check:
	cargo check --all-features

build:
	cargo build --all-features

bench:
	cargo bench

fmt:
	cargo +nightly fmt

test:
	cargo nextest run --all-features

docs-reference:
	cargo build --release -p shingetsu-cli
	./target/release/shingetsu doc dump-json --out docs/shingetsu-docs.json
	./target/release/shingetsu doc render-markdown \
		--input docs/shingetsu-docs.json \
		--out docs/reference \
		--front-matter zensical \
		--nav-fragment target/_nav_fragment.toml \
		--nav-prefix reference
	rm -f docs/shingetsu-docs.json

docs-config: docs-reference
	python3 -c "import sys; cfg = open('zensical.toml').read(); frag = open('target/_nav_fragment.toml').read().rstrip(); sys.stdout.write(cfg.replace('\"@@REFERENCE_NAV@@\"', frag))" > zensical.build.toml

docs: docs-config
	rm -rf .zensical site
	zensical build --clean -f zensical.build.toml

docs-serve: docs-config
	zensical serve -f zensical.build.toml -a 0.0.0.0:8000
