JPEG decoder fixtures contain synthetic solid colors, generated with Pillow 13.0.0:

```python
from PIL import Image
for mode, color, name, progressive in [
    ('RGB', (240, 20, 10), 'rgb', False),
    ('RGB', (240, 20, 10), 'progressive', True),
    ('L', 120, 'gray', False),
    ('CMYK', (0, 255, 255, 0), 'cmyk', False),
]:
    Image.new(mode, (16, 12), color).save(name + '.jpg', quality=95, progressive=progressive)
```
