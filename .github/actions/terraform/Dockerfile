FROM hashicorp/terraform:0.15.0

RUN apk add bash

COPY main.sh    /terraform/main.sh
COPY apply.sh   /terraform/apply.sh
COPY cleanup.sh /terraform/cleanup.sh

ENTRYPOINT [ "/terraform/main.sh" ]
