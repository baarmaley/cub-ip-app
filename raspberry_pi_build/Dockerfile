FROM ragnaroek/rust-raspberry:1.58.1

RUN sudo apt-get update && sudo apt-get install -y cmake binutils-arm-linux-gnueabi \
    && $HOME/.cargo/bin/cargo install cargo-deb

RUN echo 'strip = { path =  "arm-linux-gnueabi-strip" }'  | sudo tee -a $HOME/.cargo/config

ENTRYPOINT ["run.sh"]

CMD ["help"]


