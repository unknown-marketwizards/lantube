# LanTube

一直以来总有这样的困扰：

> 在网络上下载下来的视频（因为版权问题无法发到公共平台），需要同时在电脑和手机（平板）上播放，但又不想把视频拷贝到各个设备上，于是就有了这个项目。

---

这是一个局域网的视频服务器，可以部署在局域网内的任何（有操作系统的）设备上（甚至是树莓派）

* 电脑（Windows, Linux, Mac）
* 手机平板
* 树莓派

---

# 特性

* 使用目录树、面包屑和当前目录搜索浏览共享目录。
* 媒体库和播放队列显示视频帧缩略图。播放和缩略图取决于浏览器支持的媒体编码。
* 上传文件、新建目录，支持简体中文和英文。
* 支持自动连播、逐个手动播放和定时暂停。

# 如何构建

## 构建前端

* 安装 [Node.js](https://nodejs.org)

* 打开 终端 / PowerShell / Cmd

* Clone 本仓库

  ```bash
  git clone https://github.com/unknown-marketwizards/lantube.git
  ```

* 进入文件夹

  ```bash
  cd lantube/frontend
  ```

* 安装需要的库文件

  ```bash
  npm install
  ```

* 构建

  ```bash
  npm run build
  ```

## 构建后台

* 安装 [rust](https://www.rust-lang.org)

* 设置代理，参见[RsProxy](https://rsproxy.cn/#getStarted)

* 进入文件夹

  ```bash
  cd lantube
  ```
  
* 构建

  ```bash
  cargo build --release
  ```

## 运行
* 编辑配置文件
  ```bash
  cp config.example.toml config.toml
  vim config.toml
  ```
* 运行
  ```bash
  ./target/release/lantube
  ```

---
如果您对股票，期货，外汇等金融交易感兴趣

欢迎加入`交易奇才 Slack 交流群`，让我们一起成为更好的交易者！

![](images/qrcode.jpeg)

> 可扫码关注公众号，并回复：**加群**
