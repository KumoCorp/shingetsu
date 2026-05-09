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

DOC_CRATES := \
	-p shingetsu -p shingetsu-vm -p shingetsu-compiler \
	-p shingetsu-meta -p shingetsu-derive -p shingetsu-docgen \
	-p shingetsu-repl -p shingetsu-migrate -p shingetsu-migrate-derive

# rustdoc does not track --html-in-header / --html-before-content as inputs,
# so cargo's fingerprint cache misses edits to those files. Wipe target/doc
# before each build to guarantee the latest header is baked in.
docs-rustdoc:
	rm -rf target/doc
	RUSTDOCFLAGS="--html-in-header docs/assets/rustdoc/header.html \
		--html-before-content docs/assets/rustdoc/before-content.html" \
		cargo +nightly doc --no-deps -Zrustdoc-map $(DOC_CRATES)
	rm -rf docs/api
	mkdir -p docs/api
	cp -r target/doc/. docs/api/

docs-config: docs-reference docs-rustdoc
	python3 -c "import sys; cfg = open('zensical.toml').read(); frag = open('target/_nav_fragment.toml').read().rstrip(); sys.stdout.write(cfg.replace('\"@@REFERENCE_NAV@@\"', frag))" > zensical.build.toml

docs: docs-config
	rm -rf .zensical site
	zensical build --clean -f zensical.build.toml

docs-serve: docs-config
	zensical serve -f zensical.build.toml -a 0.0.0.0:8000
