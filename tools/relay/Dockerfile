FROM python:3.13-slim
WORKDIR /app
ENV HOST=0.0.0.0 PYTHONUNBUFFERED=1
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY relay.py .
EXPOSE 2108
CMD ["python", "relay.py", "--port", "2108"]
