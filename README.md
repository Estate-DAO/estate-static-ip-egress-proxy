Deploys nginx proxy to fly.io

Here are a few notable things:

1. no port exposed in services section to ensure that ingress to the nginx is not from the internet
2. hence, no https is needed since the communication is via 6pn private network of fly.io
3. uses `egress-ip` to gain a static IP
4. this repo is only deployed via ONE fly machine - the static IP is given to that machine.