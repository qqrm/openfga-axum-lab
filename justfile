# justfile для локального стенда openfga-axum-lab
# Установка just: https://github.com/casey/just

set shell := ["bash", "-cu"]

default:
    @just --list

# Сборка Rust-приложения
build:
    cargo build

# Проверки: форматирование, clippy, unit-тесты
check:
    cargo fmt --check
    cargo clippy --all-targets
    cargo test

# Форматирование кода
fmt:
    cargo fmt

# Запуск локального стенда (PostgreSQL, OpenFGA, bootstrap, приложение)
start:
    docker compose up -d --build

# Остановка стенда без удаления данных
stop:
    docker compose down

# Полная очистка стенда и томов
clean:
    docker compose down -v

# Логи приложения
logs:
    docker compose logs -f app

# Интеграционные тесты с PostgreSQL и OpenFGA (требуют Docker и cargo-nextest)
test:
    cargo nextest run --locked --release --profile ci \
        --no-default-features --features "${FGA_TRANSPORT:-grpc}" \
        --test system --test cache_replicas --test compose_deployment --test performance \
        --run-ignored only -E 'not test(paired_aa_calibration)' --success-output immediate

# Проверка кэша на двух репликах (требует Docker и cargo-nextest)
cache-test:
    cargo nextest run --locked --test cache_replicas --run-ignored only --success-output immediate
# Kubernetes (локально, minikube/kind)
k8s-config:
    kubectl create namespace openfga-axum-lab --dry-run=client -o yaml | kubectl apply -f -
    kubectl -n openfga-axum-lab create configmap fga-model --from-file=fga/model.json --from-file=fga/legacy.json --dry-run=client -o yaml | kubectl apply -f -

k8s-start:
    just k8s-config
    kubectl apply -f k8s/
    kubectl -n openfga-axum-lab rollout status deployment/postgres --timeout=180s
    kubectl -n openfga-axum-lab wait --for=condition=complete job/openfga-migrate --timeout=180s
    kubectl -n openfga-axum-lab rollout status deployment/openfga --timeout=180s
    kubectl -n openfga-axum-lab rollout status deployment/app --timeout=180s

k8s-port-forward:
    kubectl -n openfga-axum-lab port-forward svc/app 3000:3000

k8s-stop:
    kubectl delete namespace openfga-axum-lab --ignore-not-found

# Playwright browser E2E
e2e-install:
    cd e2e && npm install && npx playwright install chromium

e2e:
    cd e2e && npm test

# Kubernetes via Kind
kind-create:
    kind create cluster --name openfga-axum-lab

kind-load:
    kind load docker-image openfga-axum-lab:local --name openfga-axum-lab

kind-start:
    just kind-create
    just docker-build
    just kind-load
    just k8s-config
    kubectl apply -f k8s/
    kubectl -n openfga-axum-lab rollout status deployment/postgres --timeout=180s
    kubectl -n openfga-axum-lab wait --for=condition=complete job/openfga-migrate --timeout=180s
    kubectl -n openfga-axum-lab rollout status deployment/openfga --timeout=180s
    kubectl -n openfga-axum-lab rollout status deployment/app --timeout=180s

kind-stop:
    kind delete cluster --name openfga-axum-lab
