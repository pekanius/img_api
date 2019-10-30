# Тестовое задание - API сервис загрузки изображений


[![Actions Status](https://github.com/pekanius/img_api/workflows/Tests/badge.svg)](https://github.com/pekanius/img_api/actions)

REST API с единственным методом "/upload".

Обрабатывает как multipart, так и JSON запросы.

Все типы файлов, кроме картинок, фильтруются.

Файлы загружаются в папку "./imgs".

Сервис поднимается доступен по адресу localhost:8080.

# Способ использования

***Обычный***

```
git clone https://github.com/pekanius/img_api.git
cd img_api/api_service
cargo run --release
```

***Docker-compose***
```
docker-compose up -d
```

Специально для демонстрации загрузки файлов через multipart по адресу "localhost:8080" есть страничка с формой загрузки.

# Пример запроса

***Пример JSON запроса (POST /upload)***
```json
{
    "urls": [
        "https://example.com/image.jpg",
        "https://example.com/image2.jpg"
    ] 
}
```

В качестве ответа вернётся массив с именами файлов в папке /imgs.

Превью конвертируются в формат "jpg"  и имеют префикс "preview_".
