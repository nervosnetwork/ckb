#!/bin/bash

set -euo pipefail

# Path to location of terraform files
TERRAFORM_DIR=${INPUT_TERRAFORM_DIR:-${TERRAFORM_DIR}}

function apply_infrastructure () {
    cd ${TERRAFORM_DIR}
    terraform init
    terraform plan
    terraform apply -auto-approve
}

function cleanup_infrastructure () {
    cd ${TERRAFORM_DIR}
    terraform init
    terraform destroy -auto-approve
}

function cleanup_terraform_footprint () {
    rm -rf ${TERRAFORM_DIR}/.terraform
    rm -rf ${TERRAFORM_DIR}/terraform.tfstate
}

function main () {
    [ $1 ] || { echo "Wrong usage"; exit 1; }

    local command="${1}"
    shift 1
    case ${command} in
        apply )
            apply_infrastructure
            ;;
        cleanup )
            cleanup_infrastructure
            cleanup_terraform_footprint
            ;;
        * )
            echo "Wrong usage"; exit 1;
            ;;
    esac
}

main "$@"
