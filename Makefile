.PHONY: build clean db-setup db-migrate

# Docker image name
IMAGE_NAME = shade/covclaim

# Build the Docker image for linux/amd64
build:
	docker build --platform linux/amd64 -t $(IMAGE_NAME) .

# Clean up old images
clean:
	docker rmi $(IMAGE_NAME) || true

# Setup the database
db-setup:
	docker cp setup.sql boltz-postgres:/setup.sql && \
	docker exec -i boltz-postgres psql -U boltz -d boltz -f /setup.sql

# Run database migrations
db-migrate:
	DATABASE_URL=postgresql://boltz:boltz@localhost:5432/covclaim diesel migration run --migration-dir migrations_postgres

# Build everything from scratch
all: clean build db-setup db-migrate 