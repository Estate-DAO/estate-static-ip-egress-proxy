# Use the official Caddy image as the base image
FROM caddy:latest

RUN cat <<EOF > /etc/caddy/Caddyfile
{
    auto_https off
    default_sni prod.services.travelomatix.com 
}


:8080 {
# Reverse proxy to HTTPS backend
reverse_proxy  https://prod.services.travelomatix.com  {
    header_up Host {upstream_hostport}

    # transport http {
    #     tls_server_name prod.services.travelomatix.com     
    # }
}

# Rewrite paths by removing "/produrl" prefix
@rewritePath path_regexp produrl ^/produrl/(.*)$
rewrite @rewritePath /{http.regexp.produrl.1}

log {
    output stdout
    format json
}

}
EOF

# Expose necessary ports
EXPOSE 8080
# EXPOSE 443
# EXPOSE 443/udp

CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile"]
