#!/usr/bin/make
build:
	docker build -t performance-service:latest .

shell:
	docker run -it --net=host --entrypoint /bin/bash performance-service:latest
