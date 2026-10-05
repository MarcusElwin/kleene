# Build the Kleene technical report.
#
#   make          -> main.pdf (latexmk runs pdflatex and bibtex as needed)
#   make figures  -> regenerate figures/*.pdf from the SVGs under ../plots
#   make clean    -> remove build products, keep main.pdf
#   make distclean-> also remove main.pdf
#
# Needs a TeX Live with latexmk, natbib, tikz, booktabs, subcaption and
# listings (texlive-latex-extra on Debian/Ubuntu, MacTeX on macOS), and
# rsvg-convert (librsvg) for `make figures`.

LATEXMK ?= latexmk
RSVG    ?= rsvg-convert
PLOTS   ?= ../plots

OPUS_FIGS  := cost_parity learning_curve calls_vs_difficulty
HAIKU_FIGS := cost_parity learning_curve calls_vs_difficulty estimate_accuracy

.PHONY: all figures clean distclean

all: main.pdf

main.pdf: main.tex references.bib $(wildcard figures/*.pdf)
	$(LATEXMK) -pdf -interaction=nonstopmode -halt-on-error main.tex

figures: $(OPUS_FIGS:%=figures/opus_%.pdf) $(HAIKU_FIGS:%=figures/haiku_%.pdf)

figures/opus_%.pdf: $(PLOTS)/%.svg
	@mkdir -p figures
	$(RSVG) -f pdf -o $@ $<

figures/haiku_%.pdf: $(PLOTS)/haiku-2026-10-01/%.svg
	@mkdir -p figures
	$(RSVG) -f pdf -o $@ $<

clean:
	$(LATEXMK) -c main.tex
	rm -f main.bbl main.run.xml

distclean: clean
	rm -f main.pdf
