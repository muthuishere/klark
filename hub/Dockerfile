FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build
WORKDIR /src
COPY KlarkHub.csproj .
RUN dotnet restore
COPY . .
RUN dotnet publish -c Release -o /app --no-restore

FROM mcr.microsoft.com/dotnet/aspnet:10.0
WORKDIR /app
COPY --from=build /app .
ENV ASPNETCORE_URLS=http://+:8080 \
    Hub__DataDir=/data
# hub.db and machines/*.db (each machine capped at 10 MB) — keep this on a volume.
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["dotnet", "KlarkHub.dll"]
