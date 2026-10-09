# Build the Kleene technical report.
#
#   make          -> main.pdf (latexmk runs pdflatex and bibtex as needed)
#   make figures  -> regenerate figures/*.pdf from the SVGs under ../plots
#   make clean    -> remove build products, keep main.pdf
#   make distclean-> also remove main.pdf
#
# Needs a TeX Live with latexmk, natbib, tikz, booktabs, subcaption and
# listings (texlive-latex-extra on Debian/Ubuntu, MacTeX on macOS), and
# Python 3 with matplotlib for `make figures`.

LATEXMK ?= latexmk
PLOTS   ?= ../plots

.PHONY: all figures clean distclean

all: main.pdf

main.pdf: main.tex references.bib $(wildcard figures/*.pdf)
	$(LATEXMK) -pdf -interaction=nonstopmode -halt-on-error main.tex

# Every figure is drawn from the committed eval rows and plots by
# figures/make_figures.py (Python 3 with matplotlib).
figures:
	python3 figures/make_figures.py

clean:
	$(LATEXMK) -c main.tex
	rm -f main.bbl main.run.xml

distclean: clean
	rm -f main.pdf
