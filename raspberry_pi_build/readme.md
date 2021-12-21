## Raspberry pi build:
- Download libssl-dev package for arm architecture:
```shell
cd raspberry_pi_build/dep && curl -O https://deb.debian.org/debian/pool/main/o/openssl/libssl-dev_1.1.1d-0+deb10u7_armhf.deb
```
- Build docker image:
```shell
sudo docker build --rm --tag cub-ip-app-build-image .
```
- Create a deb package 
```shell
sudo docker run --network=host --rm \
--volume $(pwd):/home/cross/project \
--volume $(pwd)/raspberry_pi_build/dep:/home/cross/deb-deps \
--volume $(pwd)/raspberry_pi_build/registry:/home/cross/.cargo/registry cub-ip-app-build-image:latest deb
```