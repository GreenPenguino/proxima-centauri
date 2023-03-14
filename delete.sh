curl --header "Content-Type: application/json" \
  --data '{"command": {
                       "Delete": {
                                "id": "67e55044-10b1-426f-9247-bb680e5fe0c8"
                              }
                     },
           "signature": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
         }' \
  http://localhost:3000/command
