# Makefile — Build tutorial slides and cheatsheets for Mununu
#
# Local (requires latexmk):
#   make slides                    — compile all slide decks
#   make cheatsheets               — compile all cheatsheets
#   make all                       — compile everything
#   make slide FILE=01_basic_modeling — compile one slide deck
#   make cheat FILE=01_basic_modeling — compile one cheatsheet
#   make clean                     — remove all build artifacts
#
# Docker (reproducible builds):
#   make docker-build              — build the LaTeX Docker image (once)
#   make docker-slides             — compile all slides in Docker
#   make docker-cheatsheets        — compile all cheatsheets in Docker
#   make docker-all                — compile everything in Docker
#   make docker-slide FILE=01_basic_modeling  — compile one slide in Docker
#   make docker-cheat FILE=01_basic_modeling  — compile one cheatsheet in Docker
#   make docker-clean              — clean artifacts via Docker

LATEXMK := latexmk -pdf -interaction=nonstopmode

DOCKER_IMAGE := mununu-tutorial-latex
DOCKER_RUN   := docker run --rm -v $(CURDIR):/work $(DOCKER_IMAGE)

.PHONY: all slides cheatsheets slide cheat clean
.PHONY: docker-build docker-all docker-slides docker-cheatsheets docker-slide docker-cheat docker-clean

# --- Local targets ---

all: slides cheatsheets

# Single latexmk invocation for all files in each directory (much faster)
slides:
	cd slides && $(LATEXMK) [0-9]*.tex

cheatsheets:
	cd cheatsheet && $(LATEXMK) [0-9]*.tex

slide:
	@test -n "$(FILE)" || (echo "Usage: make slide FILE=01_basic_modeling" && exit 1)
	cd slides && $(LATEXMK) $(FILE).tex

cheat:
	@test -n "$(FILE)" || (echo "Usage: make cheat FILE=01_basic_modeling" && exit 1)
	cd cheatsheet && $(LATEXMK) $(FILE).tex

clean:
	cd slides && $(LATEXMK) -C [0-9]*.tex 2>/dev/null || true
	cd cheatsheet && $(LATEXMK) -C [0-9]*.tex 2>/dev/null || true

# --- Docker targets ---

docker-build:
	docker build -t $(DOCKER_IMAGE) .

docker-all:
	$(DOCKER_RUN) make all

docker-slides:
	$(DOCKER_RUN) make slides

docker-cheatsheets:
	$(DOCKER_RUN) make cheatsheets

docker-slide:
	@test -n "$(FILE)" || (echo "Usage: make docker-slide FILE=01_basic_modeling" && exit 1)
	$(DOCKER_RUN) make slide FILE=$(FILE)

docker-cheat:
	@test -n "$(FILE)" || (echo "Usage: make docker-cheat FILE=01_basic_modeling" && exit 1)
	$(DOCKER_RUN) make cheat FILE=$(FILE)

docker-clean:
	$(DOCKER_RUN) make clean
