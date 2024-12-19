# Use official Nginx base image
FROM nginx:mainline-bookworm

# Set the working directory to /etc/nginx
WORKDIR /etc/nginx

# Create sites-available and sites-enabled directories
RUN mkdir -p /etc/nginx/sites-available /etc/nginx/sites-enabled

RUN cat <<EOF > /etc/nginx/conf.d/fly_run_proxy.conf
server {
    server_name _;

    listen 8080;
    listen [::]:8080;

    location /test {
        proxy_pass https://ipv4.icanhazip.com;
        proxy_ssl_server_name on; # Ensure SNI support
    }

    location /test2 {
        proxy_pass https://ifconfig.me/ip;
        proxy_ssl_server_name on; # Ensure SNI support
    }
}
EOF

EXPOSE 8080

# Run Nginx in the foreground (required by Docker)
CMD ["nginx", "-g", "daemon off;"]
