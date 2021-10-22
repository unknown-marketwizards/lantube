package main

import (
	"fmt"
	"github.com/alexflint/go-arg"
	"github.com/kataras/iris/v12"
	"github.com/kataras/iris/v12/middleware/logger"
	"github.com/kataras/iris/v12/middleware/recover"
	"io"
	"io/ioutil"
	"os"
	"path"
	"strings"
)

var DataDir string

var fileTypeList = []string{".mp4", ".webm", ".wmv", ".ts", ".rmvb", ".rm", ".avi", "flv"}

func main() {
	if len(os.Args) == 1 {
		fmt.Println("Usage:", os.Args[0], "<DataDir>")
		return
	}

	DataDir = os.Args[len(os.Args)-1]
	os.Args = os.Args[:len(os.Args)-1]

	var args struct {
		Addr string `help:"listening address"`
	}

	arg.MustParse(&args)

	if args.Addr == "" {
		args.Addr = "127.0.0.1:8888"
	}

	app := iris.New()

	app.Use(recover.New())
	app.Use(logger.New())

	app.RegisterView(iris.HTML("./frontend/dist", ".html").Reload(true))
	app.Favicon("./frontend/dist/favicon.ico")

	app.HandleDir("assets", "./frontend/dist/assets")
	app.HandleDir("file", DataDir)

	app.Get("/", func(ctx iris.Context) {
		ctx.View("index.html")
	})

	app.Post("/api/explorer", func(ctx iris.Context) {
		var form = struct {
			Path string `json:"path"`
		}{}
		if err := ctx.ReadJSON(&form); err != nil {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}

		dir := path.Join(DataDir, form.Path)
		rd, err := ioutil.ReadDir(dir)
		if err != nil {
			return
		}

		var result []iris.Map
		for _, fi := range rd {
			if fi.IsDir() {
				result = append(result, iris.Map{"name": fi.Name(), "type": "dir"})
			} else {
				ext := strings.ToLower(path.Ext(fi.Name()))
				for _, t := range fileTypeList {
					if t == ext {
						result = append(result, iris.Map{"name": fi.Name(), "type": "file"})
						break
					}
				}
			}
		}

		ctx.JSON(result)
	})

	app.Post("/api/upload", func(ctx iris.Context) {
		dir := ctx.FormValue("path")
		if dir == "" {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}

		file, info, err := ctx.FormFile("file")
		if err != nil {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}

		defer file.Close()

		filePath := path.Join(DataDir, dir, info.Filename)

		out, err := os.OpenFile(filePath, os.O_WRONLY|os.O_CREATE, 0644)

		if err != nil {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}
		defer out.Close()
		io.Copy(out, file)
	})

	app.Post("/api/mkdir", func(ctx iris.Context) {
		var form = struct {
			Path string `json:"path"`
		}{}
		if err := ctx.ReadJSON(&form); err != nil {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}

		if form.Path == "" {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}

		dir := path.Join(DataDir, form.Path)
		if err := os.Mkdir(dir, os.ModePerm); err != nil {
			ctx.StatusCode(iris.StatusBadRequest)
			return
		}
	})

	app.Listen(args.Addr)
}
