docker-image:
	docker build -t hone/ruby-builder:$(STACK) -f dockerfiles/Dockerfile.$(STACK) --pull .
docker-push:
	docker push hone/ruby-builder:$(STACK)
docker-build: clean
	docker run -it -v `pwd`:/root/mount -e GIT_URL=$$GIT_URL -e STACK=$(STACK) -e RUBY_VERSION=$(RUBY_VERSION) --name ruby-build hone/ruby-builder:$(STACK) make build
	mkdir -p builds/$(STACK)
	docker cp ruby-build:/root/work/builds/$(STACK)/ruby-$(RUBY_VERSION).tgz builds/$(STACK)/ruby-$(RUBY_VERSION).tgz
clean:
	if [ $$(docker container ls -a --filter name=ruby-build | wc -l) -gt 1 ]; then docker rm ruby-build; fi
