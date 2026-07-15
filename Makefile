SHELL := /bin/sh

APP := 12306-rs
CRATE := rs12306-cli
VERSION ?= $(shell awk -F'"' '/^version = / { print $$2; exit }' crates/cli/Cargo.toml)
OS := $(shell uname -s | tr '[:upper:]' '[:lower:]')
ARCH := $(shell uname -m)

CARGO ?= cargo
DIST_DIR ?= dist
PACKAGE_NAME := $(APP)-v$(VERSION)-$(OS)-$(ARCH)
STAGE_DIR := $(DIST_DIR)/$(PACKAGE_NAME)
PACKAGE_FILE := $(DIST_DIR)/$(PACKAGE_NAME).tar.gz

LINUX_ARCH ?= amd64
LINUX_PACKAGE_NAME = $(APP)-v$(VERSION)-linux-$(LINUX_ARCH)
LINUX_STAGE_DIR = $(DIST_DIR)/$(LINUX_PACKAGE_NAME)
LINUX_EXPORT_DIR = $(DIST_DIR)/.linux-$(LINUX_ARCH)
LINUX_PACKAGE_FILE = $(DIST_DIR)/$(LINUX_PACKAGE_NAME).tar.gz

HOST ?= 127.0.0.1
PORT ?= 12306
DATABASE ?= ./data/12306-rs.sqlite

IMAGE ?= $(APP):latest
CONTAINER ?= $(APP)
DOCKER_DATA ?= $(APP)-data
API_TOKEN ?=

.DEFAULT_GOAL := help

.PHONY: help all fmt check test build release deploy package run \
	linux-binary package-linux package-linux-amd64 package-linux-arm64 \
	docker-build docker-deploy docker-stop docker-logs clean

help:
	@printf '%s\n' \
		'12306-rs 构建与部署命令' \
		'' \
		'  make check          格式检查和 Clippy' \
		'  make test           运行全部测试' \
		'  make build          构建 debug 二进制' \
		'  make release        构建 release 二进制' \
		'  make deploy         更新根目录 ./12306-rs' \
		'  make package        生成 dist/*.tar.gz 发布包' \
		'  make package-linux  生成 Linux amd64 二进制包' \
		'  make package-linux-amd64  生成 Linux amd64 包' \
		'  make package-linux-arm64  生成 Linux arm64 包' \
		'  make run            构建并启动本机 Web 服务' \
		'  make docker-build   构建 Docker 镜像' \
		'  make docker-deploy  重新部署 Docker 容器' \
		'  make docker-stop    停止 Docker 容器' \
		'  make docker-logs    跟踪 Docker 日志' \
		'  make clean          清理 Cargo 和 dist 产物' \
		'' \
		'可覆盖变量: HOST PORT DATABASE IMAGE CONTAINER DOCKER_DATA API_TOKEN LINUX_ARCH'

all: check test package

fmt:
	$(CARGO) fmt --all

check:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test:
	$(CARGO) test --workspace

build:
	$(CARGO) build -p $(CRATE)

release:
	$(CARGO) build --release -p $(CRATE)

deploy: release
	install -m 755 target/release/$(APP) ./$(APP)
	@printf '本机二进制已更新: %s/%s\n' '$(CURDIR)' '$(APP)'

package: deploy
	rm -rf '$(STAGE_DIR)'
	mkdir -p '$(STAGE_DIR)/docs'
	install -m 755 target/release/$(APP) '$(STAGE_DIR)/$(APP)'
	cp README.md '$(STAGE_DIR)/'
	cp docs/run.md '$(STAGE_DIR)/docs/'
	COPYFILE_DISABLE=1 tar -C '$(DIST_DIR)' -czf '$(PACKAGE_FILE)' '$(PACKAGE_NAME)'
	rm -rf '$(STAGE_DIR)'
	@printf '发布包已生成: %s\n' '$(PACKAGE_FILE)'

linux-binary:
	rm -rf '$(LINUX_EXPORT_DIR)'
	mkdir -p '$(LINUX_EXPORT_DIR)'
	docker buildx build \
		--platform 'linux/$(LINUX_ARCH)' \
		--target binary \
		--output 'type=local,dest=$(LINUX_EXPORT_DIR)' \
		.
	test -x '$(LINUX_EXPORT_DIR)/$(APP)'

package-linux: linux-binary
	rm -rf '$(LINUX_STAGE_DIR)'
	mkdir -p '$(LINUX_STAGE_DIR)/docs'
	install -m 755 '$(LINUX_EXPORT_DIR)/$(APP)' '$(LINUX_STAGE_DIR)/$(APP)'
	cp README.md '$(LINUX_STAGE_DIR)/'
	cp docs/run.md '$(LINUX_STAGE_DIR)/docs/'
	COPYFILE_DISABLE=1 tar -C '$(DIST_DIR)' -czf '$(LINUX_PACKAGE_FILE)' '$(LINUX_PACKAGE_NAME)'
	rm -rf '$(LINUX_STAGE_DIR)' '$(LINUX_EXPORT_DIR)'
	@printf 'Linux 发布包已生成: %s\n' '$(LINUX_PACKAGE_FILE)'

package-linux-amd64: LINUX_ARCH := amd64
package-linux-amd64: package-linux

package-linux-arm64: LINUX_ARCH := arm64
package-linux-arm64: package-linux

run: deploy
	./$(APP) --database '$(DATABASE)' serve --host '$(HOST)' --port '$(PORT)'

docker-build:
	docker build -t '$(IMAGE)' .

docker-deploy: docker-build
	@test -n '$(API_TOKEN)' || { printf '%s\n' '请设置 API_TOKEN（至少 16 个字符）'; exit 1; }
	@docker rm -f '$(CONTAINER)' >/dev/null 2>&1 || true
	docker run -d \
		--name '$(CONTAINER)' \
		--restart unless-stopped \
		-p '127.0.0.1:$(PORT):12306' \
		-e RS12306_API_TOKEN='$(API_TOKEN)' \
		-v '$(DOCKER_DATA):/data' \
		'$(IMAGE)'
	@printf 'Docker 服务已启动: http://127.0.0.1:%s\n' '$(PORT)'

docker-stop:
	docker stop '$(CONTAINER)'

docker-logs:
	docker logs -f '$(CONTAINER)'

clean:
	$(CARGO) clean
	rm -rf '$(DIST_DIR)'
