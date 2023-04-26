curl --header "Content-Type: application/json" \
  --data '{
            "modify": {
                    "destination_port": 8080,
                    "destination_ip": "127.0.0.1",
                    "id": "67e55044-10b1-426f-9247-bb680e5fe0c8"
                  }
          }' \
  http://localhost:14000/command
