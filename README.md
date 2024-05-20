# LanTube

[简体中文](README.zh.md)

I have always had such troubles：

> Videos downloaded on the Internet（cannot be posted to public platforms due to copyright issues）,
> need to be played on computer and mobile phone (tablet),
> But don’t want to copy the video to each device, so there is this project.

---

This is a LAN video server, which can be deployed on any device (with operating system) in the LAN (even Raspberry Pi)

* Computer（Windows, Linux, Mac）
* MobilePhone or tablet
* RaspberryPie

---

It is built using the following open source project

* [Vue.js](https://vuejs.org)
* [Element](https://element.eleme.cn)
* [Video.js](https://github.com/videojs/video.js)
* [Videojs-playlist](https://github.com/brightcove/videojs-playlist)
* [Videojs-playlist-ui](https://github.com/brightcove/videojs-playlist-ui)

---
![](images/screenshot.gif)

# Features

* Any LAN device can be deployed
* PlayList
* Timed play
* Multi-language support

# How to build

## Build frontend

* Install [Node.js](https://nodejs.org)

* Open Terminal / PowerShell / cmd

* Clone this repo

  ```bash
  git clone https://github.com/unknown-marketwizards/lantube.git
  ```

* Enter folder

  ```bash
  cd lantube/frontend
  ```

* Install the required library files

  ```bash
  yarn install
  ```

* Build

  ```bash
  yarn build
  ```

## Build Server

* Install [rust](https://www.rust-lang.org)

* Enter folder

  ```bash
  cd lantube
  ```

* Build

  ```bash
  cargo build --release
  ```

* Run

  ```bash
  ./target/release/lantube --addr 0.0.0.0:9000 "Directory / for / storing / videos"
  ```
---
如果您对股票，期货，外汇等金融交易感兴趣

欢迎加入`交易奇才 Slack 交流群`，让我们一起成为更好的交易者！

![](images/qrcode.jpeg)

> 可扫码关注公众号，并回复：**加群**