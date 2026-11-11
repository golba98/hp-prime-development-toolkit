import {defineConfig} from '@playwright/test';
export default defineConfig({testDir:'web-tests',timeout:20000,workers:1,use:{headless:true,viewport:{width:1000,height:900},launchOptions:{executablePath:process.env.HPDEV_CHROMIUM_EXECUTABLE||undefined}},reporter:'list'});
